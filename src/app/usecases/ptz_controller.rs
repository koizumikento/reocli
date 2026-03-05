use crate::core::model::{AxisEstimate, AxisModelParams, AxisState};

const MAX_PTZ_SPEED: i64 = 64;
const POSITION_CORRECTION_GAIN: f64 = 0.6;
const VELOCITY_CORRECTION_GAIN: f64 = 0.2;
const BIAS_CORRECTION_GAIN: f64 = 0.1;
const DEFAULT_EKF_POSITION_VAR: f64 = 4.0;
const DEFAULT_EKF_VELOCITY_VAR: f64 = 16.0;
const DEFAULT_EKF_BIAS_VAR: f64 = 4.0;
const DEFAULT_EKF_PROCESS_Q_POS: f64 = 0.15;
const DEFAULT_EKF_PROCESS_Q_VEL: f64 = 0.35;
const DEFAULT_EKF_PROCESS_Q_BIAS: f64 = 0.01;
const DEFAULT_EKF_MEASUREMENT_R: f64 = 1.0;
const EKF_MIN_MEASUREMENT_R: f64 = 0.05;
const EKF_MAX_MEASUREMENT_R: f64 = 30.0;
const EKF_MIN_Q_SCALE: f64 = 0.2;
const EKF_MAX_Q_SCALE: f64 = 8.0;
const EKF_ADAPTATION_LAMBDA: f64 = 0.08;
const EKF_NIS_UPPER: f64 = 1.6;
const EKF_NIS_LOWER: f64 = 0.7;
const EKF_NIS_HARD_GATE: f64 = 9.0;
const EKF_Q_SCALE_GROWTH: f64 = 1.08;
const EKF_Q_SCALE_DECAY: f64 = 0.97;
const EKF_CONSISTENCY_EWMA_ALPHA: f64 = 0.08;
const EKF_MEASUREMENT_HINT_MIN_SCALE: f64 = 0.75;
const EKF_MEASUREMENT_HINT_MAX_SCALE: f64 = 1.8;
const EKF_MEASUREMENT_HINT_BLEND_ALPHA: f64 = 0.35;
const EKF_MAX_RESIDUAL_VARIANCE_PROXY: f64 = EKF_MAX_MEASUREMENT_R * 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisControllerConfig {
    pub ts_sec: f64,
    pub min_position: f64,
    pub max_position: f64,
    pub stop_deadband_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisController {
    config: AxisControllerConfig,
    model: AxisModelParams,
}

impl AxisController {
    pub fn new(config: AxisControllerConfig, model: AxisModelParams) -> Self {
        Self { config, model }
    }

    pub fn update(
        &self,
        state: AxisState,
        target_position: f64,
        measured_position: f64,
    ) -> (AxisEstimate, f64) {
        let ts_sec = self.config.ts_sec.max(f64::EPSILON);
        let clipped_target =
            target_position.clamp(self.config.min_position, self.config.max_position);
        let predicted_output = output_from_state(state);
        let position_error = clipped_target - predicted_output;

        let mut normalized_u = (position_error - state.velocity).clamp(-1.0, 1.0);
        if position_error.abs() <= self.config.stop_deadband_deg {
            normalized_u = 0.0;
        }

        let predicted_state = AxisState {
            position: state.position + (ts_sec * state.velocity),
            velocity: (self.model.alpha * state.velocity) + (self.model.beta * normalized_u),
            bias: state.bias,
        };

        let innovation = measured_position - output_from_state(predicted_state);
        let corrected_state = AxisState {
            position: predicted_state.position + (POSITION_CORRECTION_GAIN * innovation),
            velocity: predicted_state.velocity + (VELOCITY_CORRECTION_GAIN * innovation / ts_sec),
            bias: predicted_state.bias + (BIAS_CORRECTION_GAIN * innovation),
        };

        (
            AxisEstimate {
                state: corrected_state,
                measured_position,
            },
            normalized_u,
        )
    }

    pub fn quantize_output(&self, normalized_u: f64) -> Option<(i8, u8)> {
        quantize_normalized_u(normalized_u, self.config.stop_deadband_deg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisEkfConfig {
    pub ts_sec: f64,
    pub q_position: f64,
    pub q_velocity: f64,
    pub q_bias: f64,
    pub r_measurement: f64,
    pub min_position: f64,
    pub max_position: f64,
    pub min_velocity: f64,
    pub max_velocity: f64,
    pub min_bias: f64,
    pub max_bias: f64,
}

impl AxisEkfConfig {
    pub fn with_default_noise(ts_sec: f64, min_position: f64, max_position: f64) -> Self {
        let ts = ts_sec.max(1e-3);
        let span = (max_position - min_position).abs().max(1.0);
        let velocity_limit = (span / ts).clamp(20.0, 240.0);
        let bias_limit = (span * 0.2).clamp(4.0, 45.0);
        Self {
            ts_sec,
            q_position: DEFAULT_EKF_PROCESS_Q_POS,
            q_velocity: DEFAULT_EKF_PROCESS_Q_VEL,
            q_bias: DEFAULT_EKF_PROCESS_Q_BIAS,
            r_measurement: DEFAULT_EKF_MEASUREMENT_R,
            min_position,
            max_position,
            min_velocity: -velocity_limit,
            max_velocity: velocity_limit,
            min_bias: -bias_limit,
            max_bias: bias_limit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisEkf {
    config: AxisEkfConfig,
    model: AxisModelParams,
    state: AxisState,
    covariance: [[f64; 3]; 3],
    adaptive_r: f64,
    adaptive_q_scale: f64,
    last_nis: f64,
    ewma_nis: f64,
    residual_variance_proxy: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisEkfSnapshot {
    pub state: AxisState,
    pub covariance: [[f64; 3]; 3],
    pub adaptive_r: f64,
    pub adaptive_q_scale: f64,
    pub last_nis: Option<f64>,
    pub ewma_nis: Option<f64>,
    pub residual_variance_proxy: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisEkfConsistency {
    pub last_nis: f64,
    pub ewma_nis: f64,
    pub residual_variance_proxy: f64,
    pub adaptive_r: f64,
}

impl AxisEkf {
    pub fn new(config: AxisEkfConfig, model: AxisModelParams, initial_measurement: f64) -> Self {
        let initial_position = initial_measurement.clamp(config.min_position, config.max_position);
        Self {
            config,
            model,
            state: AxisState {
                position: initial_position,
                velocity: 0.0,
                bias: 0.0,
            },
            covariance: [
                [DEFAULT_EKF_POSITION_VAR, 0.0, 0.0],
                [0.0, DEFAULT_EKF_VELOCITY_VAR, 0.0],
                [0.0, 0.0, DEFAULT_EKF_BIAS_VAR],
            ],
            adaptive_r: config
                .r_measurement
                .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_MEASUREMENT_R),
            adaptive_q_scale: 1.0,
            last_nis: 1.0,
            ewma_nis: 1.0,
            residual_variance_proxy: config
                .r_measurement
                .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_RESIDUAL_VARIANCE_PROXY),
        }
    }

    pub fn state(&self) -> AxisState {
        self.state
    }

    pub fn output(&self) -> f64 {
        output_from_state(self.state)
    }

    pub fn consistency(&self) -> AxisEkfConsistency {
        AxisEkfConsistency {
            last_nis: self.last_nis,
            ewma_nis: self.ewma_nis,
            residual_variance_proxy: self.residual_variance_proxy,
            adaptive_r: self.adaptive_r,
        }
    }

    pub fn apply_measurement_noise_hint(&mut self, scale: f64) {
        if !scale.is_finite() {
            return;
        }
        let bounded_scale = scale.clamp(
            EKF_MEASUREMENT_HINT_MIN_SCALE,
            EKF_MEASUREMENT_HINT_MAX_SCALE,
        );
        let alpha = EKF_MEASUREMENT_HINT_BLEND_ALPHA.clamp(0.0, 1.0);
        let target_r =
            (self.adaptive_r * bounded_scale).clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_MEASUREMENT_R);
        self.adaptive_r = ((1.0 - alpha) * self.adaptive_r + alpha * target_r)
            .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_MEASUREMENT_R);
        let target_residual = (self.residual_variance_proxy * bounded_scale)
            .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_RESIDUAL_VARIANCE_PROXY);
        self.residual_variance_proxy = ((1.0 - alpha) * self.residual_variance_proxy
            + alpha * target_residual)
            .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_RESIDUAL_VARIANCE_PROXY);
    }

    pub fn snapshot(&self) -> AxisEkfSnapshot {
        AxisEkfSnapshot {
            state: self.state,
            covariance: self.covariance,
            adaptive_r: self.adaptive_r,
            adaptive_q_scale: self.adaptive_q_scale,
            last_nis: Some(self.last_nis),
            ewma_nis: Some(self.ewma_nis),
            residual_variance_proxy: Some(self.residual_variance_proxy),
        }
    }

    pub fn from_snapshot(
        config: AxisEkfConfig,
        model: AxisModelParams,
        snapshot: AxisEkfSnapshot,
    ) -> Option<Self> {
        let last_nis_ok = snapshot.last_nis.is_none_or(f64::is_finite);
        let ewma_nis_ok = snapshot.ewma_nis.is_none_or(f64::is_finite);
        let residual_variance_ok = snapshot.residual_variance_proxy.is_none_or(f64::is_finite);
        if !is_finite_state(snapshot.state)
            || !is_finite_covariance(snapshot.covariance)
            || !snapshot.adaptive_r.is_finite()
            || !snapshot.adaptive_q_scale.is_finite()
            || !last_nis_ok
            || !ewma_nis_ok
            || !residual_variance_ok
        {
            return None;
        }

        let state = AxisState {
            position: snapshot
                .state
                .position
                .clamp(config.min_position, config.max_position),
            velocity: snapshot
                .state
                .velocity
                .clamp(config.min_velocity, config.max_velocity),
            bias: snapshot.state.bias.clamp(config.min_bias, config.max_bias),
        };
        let covariance = sanitize_covariance(snapshot.covariance);

        Some(Self {
            config,
            model,
            state,
            covariance,
            adaptive_r: snapshot
                .adaptive_r
                .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_MEASUREMENT_R),
            adaptive_q_scale: snapshot
                .adaptive_q_scale
                .clamp(EKF_MIN_Q_SCALE, EKF_MAX_Q_SCALE),
            last_nis: snapshot.last_nis.unwrap_or(1.0).clamp(0.0, 1_000.0),
            ewma_nis: snapshot.ewma_nis.unwrap_or(1.0).clamp(0.0, 1_000.0),
            residual_variance_proxy: snapshot
                .residual_variance_proxy
                .unwrap_or(snapshot.adaptive_r)
                .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_RESIDUAL_VARIANCE_PROXY),
        })
    }

    pub fn update(&mut self, control_u: f64, measured_position: f64) -> AxisEstimate {
        self.update_with_dt(control_u, measured_position, self.config.ts_sec)
    }

    pub fn update_with_dt(
        &mut self,
        control_u: f64,
        measured_position: f64,
        dt_sec: f64,
    ) -> AxisEstimate {
        let u = control_u.clamp(-1.0, 1.0);
        let base_ts = self.config.ts_sec.max(1e-3);
        let ts = if dt_sec.is_finite() {
            dt_sec.clamp(base_ts * 0.25, base_ts * 4.0)
        } else {
            base_ts
        };
        let alpha = self.model.alpha;
        let beta = self.model.beta;

        let predicted_state = AxisState {
            position: self.state.position + ts * self.state.velocity,
            velocity: alpha * self.state.velocity + beta * u,
            bias: self.state.bias,
        };

        let a = [[1.0, ts, 0.0], [0.0, alpha, 0.0], [0.0, 0.0, 1.0]];
        let q_scale = self
            .adaptive_q_scale
            .clamp(EKF_MIN_Q_SCALE, EKF_MAX_Q_SCALE);
        let q_time_scale = (ts / base_ts).clamp(0.25, 4.0);
        let p_pred = add_3x3(
            mul_3x3(mul_3x3(a, self.covariance), transpose_3x3(a)),
            [
                [
                    self.config.q_position.max(1e-6) * q_scale * q_time_scale,
                    0.0,
                    0.0,
                ],
                [
                    0.0,
                    self.config.q_velocity.max(1e-6) * q_scale * q_time_scale,
                    0.0,
                ],
                [
                    0.0,
                    0.0,
                    self.config.q_bias.max(1e-8) * q_scale * q_time_scale,
                ],
            ],
        );

        let innovation = measured_position - output_from_state(predicted_state);
        let h = [1.0, 0.0, 1.0];
        let ph_t = mul_3x3_3x1(p_pred, h);
        let measurement_r = self
            .adaptive_r
            .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_MEASUREMENT_R);
        let s_raw = dot_3(h, ph_t) + measurement_r.max(1e-6);
        let s = if s_raw.is_finite() {
            s_raw.max(1e-6)
        } else {
            1e-6
        };
        let residual_energy = innovation * innovation;
        let nis = if residual_energy.is_finite() {
            residual_energy / s
        } else {
            f64::INFINITY
        };
        let measurement_rejected = !nis.is_finite() || nis > EKF_NIS_HARD_GATE;

        let (corrected_state, p_corr) = if measurement_rejected {
            (
                AxisState {
                    position: predicted_state
                        .position
                        .clamp(self.config.min_position, self.config.max_position),
                    velocity: predicted_state
                        .velocity
                        .clamp(self.config.min_velocity, self.config.max_velocity),
                    bias: predicted_state
                        .bias
                        .clamp(self.config.min_bias, self.config.max_bias),
                },
                p_pred,
            )
        } else {
            let gain = scale_3(ph_t, 1.0 / s);
            let corrected_state = AxisState {
                position: (predicted_state.position + gain[0] * innovation)
                    .clamp(self.config.min_position, self.config.max_position),
                velocity: (predicted_state.velocity + gain[1] * innovation)
                    .clamp(self.config.min_velocity, self.config.max_velocity),
                bias: (predicted_state.bias + gain[2] * innovation)
                    .clamp(self.config.min_bias, self.config.max_bias),
            };
            let kh = outer_3x3(gain, h);
            let p_corr = mul_3x3(sub_3x3(identity_3(), kh), p_pred);
            (corrected_state, p_corr)
        };

        self.state = corrected_state;
        self.covariance = p_corr;
        self.adapt_noise(innovation, s);

        AxisEstimate {
            state: corrected_state,
            measured_position,
        }
    }

    pub fn reanchor(&mut self, measured_position: f64) {
        self.state = AxisState {
            position: measured_position.clamp(self.config.min_position, self.config.max_position),
            velocity: 0.0,
            bias: 0.0,
        };
        self.covariance = [
            [DEFAULT_EKF_POSITION_VAR, 0.0, 0.0],
            [0.0, DEFAULT_EKF_VELOCITY_VAR, 0.0],
            [0.0, 0.0, DEFAULT_EKF_BIAS_VAR],
        ];
    }

    fn adapt_noise(&mut self, innovation: f64, innovation_variance: f64) {
        let residual_energy = if innovation.is_finite() {
            let squared = innovation * innovation;
            if squared.is_finite() {
                squared.max(0.0)
            } else {
                f64::MAX
            }
        } else {
            f64::MAX
        };
        let safe_innovation_variance = if innovation_variance.is_finite() {
            innovation_variance.max(1e-6)
        } else {
            1e-6
        };
        let lambda = EKF_ADAPTATION_LAMBDA;

        let nis = (residual_energy / safe_innovation_variance).clamp(0.0, 1_000.0);
        self.last_nis = nis;
        let ewma_nis =
            (1.0 - EKF_CONSISTENCY_EWMA_ALPHA) * self.ewma_nis + EKF_CONSISTENCY_EWMA_ALPHA * nis;
        self.ewma_nis = if ewma_nis.is_finite() {
            ewma_nis.clamp(0.0, 1_000.0)
        } else {
            1_000.0
        };
        let residual_energy = residual_energy.clamp(0.0, EKF_MAX_RESIDUAL_VARIANCE_PROXY);
        let residual_variance_proxy =
            (1.0 - lambda) * self.residual_variance_proxy + lambda * residual_energy;
        self.residual_variance_proxy = if residual_variance_proxy.is_finite() {
            residual_variance_proxy.clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_RESIDUAL_VARIANCE_PROXY)
        } else {
            EKF_MAX_RESIDUAL_VARIANCE_PROXY
        };
        self.adaptive_r = self
            .residual_variance_proxy
            .clamp(EKF_MIN_MEASUREMENT_R, EKF_MAX_MEASUREMENT_R);

        if nis > EKF_NIS_UPPER {
            self.adaptive_q_scale = (self.adaptive_q_scale * EKF_Q_SCALE_GROWTH)
                .clamp(EKF_MIN_Q_SCALE, EKF_MAX_Q_SCALE);
        } else if nis < EKF_NIS_LOWER {
            self.adaptive_q_scale =
                (self.adaptive_q_scale * EKF_Q_SCALE_DECAY).clamp(EKF_MIN_Q_SCALE, EKF_MAX_Q_SCALE);
        }
    }
}

fn is_finite_state(state: AxisState) -> bool {
    state.position.is_finite() && state.velocity.is_finite() && state.bias.is_finite()
}

fn is_finite_covariance(covariance: [[f64; 3]; 3]) -> bool {
    covariance
        .iter()
        .all(|row| row.iter().all(|value| value.is_finite()))
}

fn sanitize_covariance(covariance: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut fixed = covariance;
    for (index, row) in fixed.iter_mut().enumerate() {
        row[index] = row[index].abs().max(1e-6);
    }
    fixed
}

pub fn quantize_normalized_u(normalized_u: f64, stop_deadband: f64) -> Option<(i8, u8)> {
    if !normalized_u.is_finite() {
        return None;
    }

    let u = normalized_u.clamp(-1.0, 1.0);
    let deadband = if stop_deadband.is_finite() {
        stop_deadband.abs().min(1.0)
    } else {
        0.0
    };

    if u.abs() <= deadband {
        return None;
    }

    let direction_sign = if u.is_sign_negative() { -1 } else { 1 };
    let speed = ((u.abs() * MAX_PTZ_SPEED as f64).round() as i64).clamp(1, MAX_PTZ_SPEED) as u8;
    Some((direction_sign, speed))
}

fn output_from_state(state: AxisState) -> f64 {
    state.position + state.bias
}

fn identity_3() -> [[f64; 3]; 3] {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

fn transpose_3x3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

fn mul_3x3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = a[row][0] * b[0][col] + a[row][1] * b[1][col] + a[row][2] * b[2][col];
        }
    }
    out
}

fn mul_3x3_3x1(a: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
        a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
        a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
    ]
}

fn add_3x3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = a[row][col] + b[row][col];
        }
    }
    out
}

fn sub_3x3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = a[row][col] - b[row][col];
        }
    }
    out
}

fn dot_3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn scale_3(v: [f64; 3], s: f64) -> [f64; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn outer_3x3(a: [f64; 3], b: [f64; 3]) -> [[f64; 3]; 3] {
    [
        [a[0] * b[0], a[0] * b[1], a[0] * b[2]],
        [a[1] * b[0], a[1] * b[1], a[1] * b[2]],
        [a[2] * b[0], a[2] * b[1], a[2] * b[2]],
    ]
}

#[cfg(test)]
mod tests;
