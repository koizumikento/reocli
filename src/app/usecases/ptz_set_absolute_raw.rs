use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::usecases::ptz_calibrate_auto::{StoredCalibration, load_saved_params_for_device};
use crate::app::usecases::ptz_controller::{AxisEkf, AxisEkfConfig, AxisEkfSnapshot};
use crate::app::usecases::ptz_deadband::{
    PositionBand, classify_position_band, scale_directional_deadband,
};
use crate::app::usecases::ptz_get_absolute_raw::{PtzRawPosition, map_status_to_raw_position};
use crate::app::usecases::ptz_pulse_lut::{AxisDirection, AxisPulseLut};
use crate::app::usecases::ptz_settle_gate::{
    CompletionGateCapabilities, PositionSettlingTracker, completion_gate_allows_success,
};
use crate::app::usecases::ptz_transport;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{AxisModelParams, NumericRange, PtzDirection};
use crate::interfaces::runtime::{self, PtzBackend};
use crate::reolink::client::Client;
use crate::reolink::onvif::OnvifPtzConfigurationOptions;
use crate::reolink::{device, ptz};

const EKF_TS_SEC: f64 = 0.08;
const EKF_STATE_SCHEMA_VERSION: u32 = 1;
const MIN_ADAPTIVE_UPDATES: usize = 2;
const REQUIRED_STABLE_STEPS: usize = 2;
const SETTLE_STEP_MS: u64 = 60;
const TIMEOUT_RETRY_BUDGET_MIN_MS: u64 = 18_000;
const TIMEOUT_RETRY_BUDGET_MAX_MS: u64 = 36_000;
const ADAPTIVE_TIMEOUT_RTT_ALPHA: f64 = 1.0 / 8.0;
const ADAPTIVE_TIMEOUT_RTT_BETA: f64 = 1.0 / 4.0;
const ADAPTIVE_TIMEOUT_RTTVAR_GAIN: f64 = 4.0;
const ADAPTIVE_TIMEOUT_MIN_SAMPLE_COUNT: usize = 2;
const ADAPTIVE_TIMEOUT_MAX_SLACK_MS: u64 = 8_000;
const ADAPTIVE_TIMEOUT_MAX_SAMPLE_MS: f64 = 10_000.0;
const MIN_CONTROL_PULSE_MS: u64 = 0;
const MAX_CONTROL_PULSE_MS: u64 = 220;
const ACTIVE_COMMAND_MIN_PULSE_MS: u64 = 8;
const MICRO_CONTROL_ERROR_COUNT: f64 = 90.0;
const FINE_CONTROL_ERROR_COUNT: f64 = 180.0;
const COARSE_CONTROL_ERROR_COUNT: f64 = 320.0;
const FINE_PHASE_ENTRY_ERROR_COUNT: f64 = 240.0;
const PULSE_LUT_ENTRY_ERROR_COUNT: f64 = 360.0;
const PULSE_LUT_TARGET_GAIN: f64 = 0.55;
const PULSE_LUT_TARGET_MIN_COUNT: f64 = 4.0;
const PULSE_LUT_TARGET_MAX_COUNT: f64 = 110.0;
const PULSE_LUT_MIN_MS: u64 = 10;
const PULSE_LUT_MAX_MS: u64 = 140;
const NEAR_TARGET_SPEED1_ENTRY_ERROR_COUNT: f64 = 420.0;
const NEAR_TARGET_SPEED1_MAX_PULSE_MS: u64 = 45;
const NEAR_TARGET_TILT_MICRO_ERROR_COUNT: f64 = 48.0;
const SECONDARY_AXIS_STRICT_ENDGAME_ERROR_COUNT: f64 = NEAR_TARGET_SPEED1_ENTRY_ERROR_COUNT;
const FINE_RELATIVE_STEP_GAIN: f64 = 0.55;
const FINE_RELATIVE_STEP_MIN_COUNT: f64 = 4.0;
const FINE_RELATIVE_STEP_MAX_COUNT: f64 = 96.0;
const FINE_FEEDFORWARD_GAIN: f64 = 0.28;
const FINE_FEEDFORWARD_MAX_COUNT: f64 = 72.0;
const BACKEND_COMPLETION_MIN_AGE_MS: u64 = 120;
const BACKEND_POSITION_STABLE_REQUIRED_STEPS: usize = 2;
const BACKEND_POSITION_STABLE_REQUIRED_STEPS_FALLBACK: usize = 4;
const BACKEND_POSITION_STABLE_TOLERANCE_RATIO: f64 = 0.35;
const BACKEND_POSITION_STABLE_MIN_COUNT: f64 = 2.0;
const BACKEND_POSITION_STABLE_MAX_COUNT: f64 = 24.0;
const REVERSAL_GUARD_MULTIPLIER: f64 = 4.0;
const REVERSAL_GUARD_MIN_COUNT: f64 = 40.0;
const REVERSAL_GUARD_MOMENTUM_MIN_SCALE: f64 = 0.6;
const REVERSAL_GUARD_NEAR_TARGET_RATIO: f64 = 1.8;
const REVERSAL_GUARD_NEAR_TARGET_MIN_COUNT: f64 = 26.0;
const REVERSAL_GUARD_NEAR_TARGET_MIN_SCALE: f64 = 0.24;
const REVERSAL_GUARD_MICRO_FLOOR_MIN_SCALE: f64 = 0.25;
const REVERSAL_GUARD_MICRO_FLOOR_MAX_COUNT: f64 = 14.0;
const DUAL_AXIS_DOMINANCE_RATIO: f64 = 1.2;
const TIE_BREAK_CLOSE_ERROR_COUNT: f64 = 320.0;
const DUAL_AXIS_CLOSE_NO_PROGRESS_TOGGLE_STEPS: usize = 3;
const DUAL_AXIS_CLOSE_PROGRESS_EPS_COUNT: f64 = 2.0;
const TILT_EDGE_CONTROL_MARGIN_COUNT: f64 = 120.0;
const TILT_EDGE_CONTROL_MAX_PULSE_MS: u64 = 20;
const TILT_EDGE_CONTROL_SPEED_CAP: u8 = 1;
const PAN_REVERSAL_MICRO_ERROR_COUNT: f64 = 200.0;
const PAN_REVERSAL_MICRO_MAX_PULSE_MS: u64 = 10;
const FAILURE_DIAG_PAN_EDGE_MARGIN_RATIO: f64 = 0.03;
const FAILURE_DIAG_PAN_EDGE_MARGIN_MIN_COUNT: f64 = 40.0;
const FAILURE_DIAG_PAN_EDGE_MARGIN_MAX_COUNT: f64 = 320.0;
const FAILURE_DIAG_STALE_DELTA_EPS_COUNT: f64 = 1.0;
const FAILURE_DIAG_STALE_STREAK_MIN: usize = 2;
const FAILURE_DIAG_AXIS_SWAP_DOMINANCE_RATIO: f64 = 1.6;
const FAILURE_DIAG_MODEL_MISMATCH_MIN_COUNT: f64 = 20.0;
const FAILURE_DIAG_MODEL_MISMATCH_RATIO: f64 = 0.8;
const ONLINE_BETA_EWMA_ALPHA: f64 = 0.22;
const ONLINE_BETA_MIN_CONTROL_U: f64 = 0.08;
const ONLINE_BETA_OUTLIER_LOW_RATIO: f64 = 0.35;
const ONLINE_BETA_OUTLIER_HIGH_RATIO: f64 = 2.8;
const ONLINE_BETA_SAMPLE_MIN_COUNT: f64 = 4.0;
const ONLINE_BETA_SAMPLE_SPAN_RATIO_MAX: f64 = 0.32;
const OSCILLATION_REVERSAL_THRESHOLD: usize = 4;
const OSCILLATION_DETECT_RANGE_MULTIPLIER: f64 = 4.0;
const OSCILLATION_MIN_DETECT_COUNT: f64 = 120.0;
const OSCILLATION_TOLERANCE_RELAX_RATIO: f64 = 0.12;
const OSCILLATION_TOLERANCE_RELAX_MAX_COUNT: f64 = 24.0;
const OSCILLATION_DAMPING_REVERSAL_SUM: usize = 3;
const OSCILLATION_DAMPING_PULSE_MS_MAX: u64 = 20;
const OSCILLATION_DAMPING_SUCCESS_BAND_MULTIPLIER: f64 = 3.0;
const MODEL_MISMATCH_RECOVERY_STEPS: usize = 3;
const CALIBRATION_DEADBAND_HINT_MAX_COUNT: f64 = 200.0;
const CALIBRATION_GUARD_DEADBAND_RATIO: f64 = 0.45;
const SUCCESS_TOLERANCE_DEADBAND_MARGIN_RATIO: f64 = 0.3;
const SUCCESS_TOLERANCE_DEADBAND_MAX_COUNT: f64 = 60.0;

const DEFAULT_PAN_MIN_COUNT: f64 = 0.0;
const DEFAULT_PAN_MAX_COUNT: f64 = 7360.0;
const DEFAULT_TILT_MIN_COUNT: f64 = 0.0;
const DEFAULT_TILT_MAX_COUNT: f64 = 1240.0;
const EKF_POSITION_MARGIN_COUNT: f64 = 120.0;
const EKF_VELOCITY_RATIO: f64 = 0.35;
const EKF_MIN_VELOCITY_COUNT_PER_SEC: f64 = 120.0;
const EKF_MAX_VELOCITY_COUNT_PER_SEC: f64 = 20_000.0;
const EKF_MAX_BIAS_RATIO: f64 = 0.08;
const EKF_MIN_BIAS_COUNT: f64 = 20.0;
const EKF_MAX_BIAS_COUNT: f64 = 3_000.0;
const MODEL_ALPHA_DEFAULT: f64 = 0.9;
const MODEL_BETA_RATIO: f64 = 0.03;
const MODEL_BETA_MIN: f64 = 20.0;
const MODEL_BETA_MAX: f64 = 600.0;
const EKF_Q_POSITION_SPAN_RATIO: f64 = 0.0012;
const EKF_Q_VELOCITY_SPAN_RATIO: f64 = 0.0035;
const EKF_Q_BIAS_SPAN_RATIO: f64 = 0.0008;
const EKF_R_MEASUREMENT_SPAN_RATIO: f64 = 0.0005;
const EKF_MIN_Q_POSITION: f64 = 0.5;
const EKF_MAX_Q_POSITION: f64 = 36.0;
const EKF_MIN_Q_VELOCITY: f64 = 2.0;
const EKF_MAX_Q_VELOCITY: f64 = 220.0;
const EKF_MIN_Q_BIAS: f64 = 0.05;
const EKF_MAX_Q_BIAS: f64 = 24.0;
const EKF_MIN_R_MEASUREMENT: f64 = 0.2;
const EKF_MAX_R_MEASUREMENT: f64 = 30.0;
const SUCCESS_LATCH_MIN_SETTLING_STEPS: usize = 1;
const SUCCESS_LATCH_CURRENT_NORM_MAX: f64 = 0.04;
const SUCCESS_LATCH_STAGNATION_MIN_BEST_AGE_STEPS: usize = 6;
const SUCCESS_LATCH_STAGNATION_NEAR_MISS_MARGIN_COUNT: f64 = 8.0;
const STRICT_COMPLETION_GRACE_STEPS: usize = 3;
const OSCILLATION_STABLE_STEP_REDUCTION_REVERSAL_SUM: usize = 6;
const SECONDARY_AXIS_INTERLEAVE_INTERVAL_MIN: usize = 1;
const SECONDARY_AXIS_INTERLEAVE_INTERVAL_MAX: usize = 3;
const EDGE_PUSH_LOCKOUT_TRIGGER_STREAK: usize = 2;
const EDGE_PUSH_LOCKOUT_HOLD_STEPS: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredAxisEkfState {
    position: f64,
    velocity: f64,
    bias: f64,
    covariance: [[f64; 3]; 3],
    #[serde(default)]
    adaptive_r: Option<f64>,
    #[serde(default)]
    adaptive_q_scale: Option<f64>,
    #[serde(default)]
    last_nis: Option<f64>,
    #[serde(default)]
    ewma_nis: Option<f64>,
    #[serde(default)]
    residual_variance_proxy: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredPtzEkfState {
    schema_version: u32,
    state_key: String,
    channel: u8,
    updated_at: u64,
    last_pan_u: f64,
    last_tilt_u: f64,
    pan: StoredAxisEkfState,
    tilt: StoredAxisEkfState,
    #[serde(default)]
    pan_positive_beta: Option<f64>,
    #[serde(default)]
    pan_negative_beta: Option<f64>,
    #[serde(default)]
    tilt_positive_beta: Option<f64>,
    #[serde(default)]
    tilt_negative_beta: Option<f64>,
    #[serde(default)]
    pan_positive_counts_per_ms: Option<f64>,
    #[serde(default)]
    pan_negative_counts_per_ms: Option<f64>,
    #[serde(default)]
    pan_positive_edge_counts_per_ms: Option<f64>,
    #[serde(default)]
    pan_negative_edge_counts_per_ms: Option<f64>,
    #[serde(default)]
    tilt_positive_counts_per_ms: Option<f64>,
    #[serde(default)]
    tilt_negative_counts_per_ms: Option<f64>,
    #[serde(default)]
    tilt_positive_edge_counts_per_ms: Option<f64>,
    #[serde(default)]
    tilt_negative_edge_counts_per_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct BestObservedState {
    pan_count: i64,
    tilt_count: i64,
    pan_abs_error: i64,
    tilt_abs_error: i64,
}

#[derive(Debug, Clone, Copy)]
struct AxisDeadbandHints {
    pan_count: f64,
    tilt_count: f64,
    pan_positive_count: f64,
    pan_negative_count: f64,
    tilt_positive_count: f64,
    tilt_negative_count: f64,
}

impl AxisDeadbandHints {
    fn pan_for_error(self, error: f64) -> f64 {
        if error >= 0.0 {
            self.pan_positive_count
        } else {
            self.pan_negative_count
        }
    }

    fn tilt_for_error(self, error: f64) -> f64 {
        if error >= 0.0 {
            self.tilt_positive_count
        } else {
            self.tilt_negative_count
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlAxis {
    Pan,
    Tilt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingPulseObservation {
    axis: ControlAxis,
    direction: AxisDirection,
    pulse_ms: u64,
    edge_band: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FailureModeCounters {
    edge_saturation_hits: usize,
    model_mismatch_hits: usize,
    axis_swap_lag_hits: usize,
    stale_status_hits: usize,
}

#[derive(Debug, Clone, Copy)]
struct AxisOnlineGainTracker {
    positive_beta: f64,
    negative_beta: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct DualAxisInterleaveState {
    dominant_axis: Option<ControlAxis>,
    dominant_streak: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct AdaptiveTimeoutRttEstimator {
    srtt_ms: Option<f64>,
    rttvar_ms: Option<f64>,
    last_sample_ms: Option<f64>,
    sample_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct AdaptiveTimeoutBudget {
    base_timeout_ms: u64,
    raw_slack_ms: u64,
    slack_cap_ms: u64,
    applied_slack_ms: u64,
    effective_timeout_ms: u64,
    srtt_ms: f64,
    rttvar_ms: f64,
    last_sample_ms: f64,
    sample_count: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct EdgePushDirectionLockout {
    risky_streak: usize,
    lockout_steps: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct EdgePushLockoutState {
    left: EdgePushDirectionLockout,
    right: EdgePushDirectionLockout,
    up: EdgePushDirectionLockout,
    down: EdgePushDirectionLockout,
}

#[derive(Debug, Clone, Copy)]
struct EdgePushContext {
    pan_measure: f64,
    pan_min_count: f64,
    pan_max_count: f64,
    pan_span: f64,
    pan_error_measured: f64,
    pan_success_tolerance: f64,
    tilt_measure: f64,
    tilt_min_count: f64,
    tilt_max_count: f64,
    tilt_error_measured: f64,
    tilt_success_tolerance: f64,
}

#[derive(Debug, Clone, Copy)]
struct OnlineLearningState<'a> {
    pan_gain_tracker: &'a AxisOnlineGainTracker,
    tilt_gain_tracker: &'a AxisOnlineGainTracker,
    pan_pulse_lut: &'a AxisPulseLut,
    tilt_pulse_lut: &'a AxisPulseLut,
    last_pan_u: f64,
    last_tilt_u: f64,
}

pub fn execute(
    client: &Client,
    channel: u8,
    target_pan_count: i64,
    target_tilt_count: i64,
    tolerance_count: i64,
    timeout_ms: u64,
) -> AppResult<PtzRawPosition> {
    validate_inputs(tolerance_count, timeout_ms)?;
    let force_cgi_pulse_transport = should_force_cgi_pulse_transport(client, channel);

    let (state_key, state_path) = ekf_state_identity(client, channel);
    let operation_result = run_closed_loop_with_timeout_retry(
        client,
        channel,
        target_pan_count,
        target_tilt_count,
        tolerance_count,
        timeout_ms,
        force_cgi_pulse_transport,
        &state_key,
        &state_path,
    );

    finalize_with_best_effort_stop(client, channel, force_cgi_pulse_transport, operation_result)
}

#[allow(clippy::too_many_arguments)]
fn run_closed_loop_with_timeout_retry(
    client: &Client,
    channel: u8,
    target_pan_count: i64,
    target_tilt_count: i64,
    tolerance_count: i64,
    timeout_ms: u64,
    force_cgi_pulse_transport: bool,
    state_key: &str,
    state_path: &Path,
) -> AppResult<PtzRawPosition> {
    let initial = run_closed_loop(
        client,
        channel,
        target_pan_count,
        target_tilt_count,
        tolerance_count,
        timeout_ms,
        force_cgi_pulse_transport,
        Instant::now() + Duration::from_millis(timeout_ms),
        state_key,
        state_path,
    );
    let initial_error = match initial {
        Ok(result) => return Ok(result),
        Err(error) => error,
    };

    if !should_retry_after_timeout(&initial_error) {
        return Err(initial_error);
    }

    let retry_timeout_ms = timeout_retry_budget_ms(timeout_ms);
    let retry = run_closed_loop(
        client,
        channel,
        target_pan_count,
        target_tilt_count,
        tolerance_count,
        retry_timeout_ms,
        force_cgi_pulse_transport,
        Instant::now() + Duration::from_millis(retry_timeout_ms),
        state_key,
        state_path,
    );
    match retry {
        Ok(result) => Ok(result),
        Err(retry_error) => Err(merge_retry_timeout_errors(&initial_error, &retry_error)),
    }
}

fn merge_retry_timeout_errors(initial_error: &AppError, retry_error: &AppError) -> AppError {
    let initial_modes = parse_failure_mode_counters(&initial_error.message).unwrap_or_default();
    let retry_modes = parse_failure_mode_counters(&retry_error.message).unwrap_or_default();
    let max_modes = max_failure_mode_counters(initial_modes, retry_modes);
    AppError::new(
        retry_error.kind.clone(),
        format!(
            "initial_timeout='{}'; retry_timeout='{}'; initial_failure_modes={}; retry_failure_modes={}; failure_modes_max={}",
            initial_error.message,
            retry_error.message,
            format_failure_mode_counters(initial_modes),
            format_failure_mode_counters(retry_modes),
            format_failure_mode_counters(max_modes),
        ),
    )
}

fn should_retry_after_timeout(error: &AppError) -> bool {
    error.kind == ErrorKind::UnexpectedResponse
        && error.message.contains("set_absolute_raw timeout")
}

fn timeout_retry_budget_ms(timeout_ms: u64) -> u64 {
    let scaled = timeout_ms.saturating_mul(3);
    scaled.clamp(TIMEOUT_RETRY_BUDGET_MIN_MS, TIMEOUT_RETRY_BUDGET_MAX_MS)
}

impl AdaptiveTimeoutRttEstimator {
    fn observe(&mut self, sample: Duration) {
        let sample_ms = (sample.as_secs_f64() * 1000.0).clamp(1.0, ADAPTIVE_TIMEOUT_MAX_SAMPLE_MS);
        if !sample_ms.is_finite() {
            return;
        }
        self.last_sample_ms = Some(sample_ms);
        self.sample_count = self.sample_count.saturating_add(1);

        let (Some(srtt_ms), Some(rttvar_ms)) = (self.srtt_ms, self.rttvar_ms) else {
            self.srtt_ms = Some(sample_ms);
            self.rttvar_ms = Some((sample_ms / 2.0).max(0.0));
            return;
        };

        let deviation_ms = (srtt_ms - sample_ms).abs();
        let updated_rttvar_ms = (1.0 - ADAPTIVE_TIMEOUT_RTT_BETA) * rttvar_ms
            + ADAPTIVE_TIMEOUT_RTT_BETA * deviation_ms;
        let updated_srtt_ms =
            (1.0 - ADAPTIVE_TIMEOUT_RTT_ALPHA) * srtt_ms + ADAPTIVE_TIMEOUT_RTT_ALPHA * sample_ms;
        self.srtt_ms = Some(updated_srtt_ms.max(0.0));
        self.rttvar_ms = Some(updated_rttvar_ms.max(0.0));
    }
}

fn adaptive_timeout_slack_cap_ms(base_timeout_ms: u64) -> u64 {
    (base_timeout_ms / 2).clamp(1, ADAPTIVE_TIMEOUT_MAX_SLACK_MS)
}

fn adaptive_timeout_budget(
    base_timeout_ms: u64,
    estimator: AdaptiveTimeoutRttEstimator,
) -> AdaptiveTimeoutBudget {
    let srtt_ms = estimator.srtt_ms.unwrap_or(0.0).max(0.0);
    let rttvar_ms = estimator.rttvar_ms.unwrap_or(0.0).max(0.0);
    let raw_slack_ms = if estimator.sample_count >= ADAPTIVE_TIMEOUT_MIN_SAMPLE_COUNT {
        let candidate = (ADAPTIVE_TIMEOUT_RTTVAR_GAIN * rttvar_ms).round();
        if candidate.is_finite() && candidate > 0.0 {
            candidate as u64
        } else {
            0
        }
    } else {
        0
    };
    let slack_cap_ms = adaptive_timeout_slack_cap_ms(base_timeout_ms);
    let applied_slack_ms = raw_slack_ms.min(slack_cap_ms);
    AdaptiveTimeoutBudget {
        base_timeout_ms,
        raw_slack_ms,
        slack_cap_ms,
        applied_slack_ms,
        effective_timeout_ms: base_timeout_ms.saturating_add(applied_slack_ms),
        srtt_ms,
        rttvar_ms,
        last_sample_ms: estimator.last_sample_ms.unwrap_or(0.0).max(0.0),
        sample_count: estimator.sample_count,
    }
}

fn adaptive_timeout_deadline(base_deadline: Instant, budget: AdaptiveTimeoutBudget) -> Instant {
    base_deadline
        .checked_add(Duration::from_millis(budget.applied_slack_ms))
        .unwrap_or(base_deadline)
}

fn parse_failure_mode_counters(message: &str) -> Option<FailureModeCounters> {
    let start = message.rfind("failure_modes=(")? + "failure_modes=(".len();
    let end = start + message[start..].find(')')?;
    let tuple = &message[start..end];

    let mut counters = FailureModeCounters::default();
    let mut parsed_count = 0usize;
    for entry in tuple.split(',') {
        let (key, value) = entry.split_once(':')?;
        let value = value.trim().parse::<usize>().ok()?;
        match key.trim() {
            "edge" => {
                counters.edge_saturation_hits = value;
                parsed_count += 1;
            }
            "model" => {
                counters.model_mismatch_hits = value;
                parsed_count += 1;
            }
            "axis_swap" => {
                counters.axis_swap_lag_hits = value;
                parsed_count += 1;
            }
            "stale" => {
                counters.stale_status_hits = value;
                parsed_count += 1;
            }
            _ => return None,
        }
    }

    if parsed_count == 4 {
        Some(counters)
    } else {
        None
    }
}

fn max_failure_mode_counters(
    left: FailureModeCounters,
    right: FailureModeCounters,
) -> FailureModeCounters {
    FailureModeCounters {
        edge_saturation_hits: left.edge_saturation_hits.max(right.edge_saturation_hits),
        model_mismatch_hits: left.model_mismatch_hits.max(right.model_mismatch_hits),
        axis_swap_lag_hits: left.axis_swap_lag_hits.max(right.axis_swap_lag_hits),
        stale_status_hits: left.stale_status_hits.max(right.stale_status_hits),
    }
}

fn format_failure_mode_counters(counters: FailureModeCounters) -> String {
    format!(
        "(edge:{},model:{},axis_swap:{},stale:{})",
        counters.edge_saturation_hits,
        counters.model_mismatch_hits,
        counters.axis_swap_lag_hits,
        counters.stale_status_hits,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_closed_loop(
    client: &Client,
    channel: u8,
    target_pan_count: i64,
    target_tilt_count: i64,
    tolerance_count: i64,
    timeout_ms: u64,
    force_cgi_pulse_transport: bool,
    deadline: Instant,
    state_key: &str,
    state_path: &Path,
) -> AppResult<PtzRawPosition> {
    let mut timeout_rtt_estimator = AdaptiveTimeoutRttEstimator::default();
    let status_with_ranges_started_at = Instant::now();
    let status_with_ranges = ptz::get_ptz_status(client, channel).ok();
    timeout_rtt_estimator
        .observe(Instant::now().saturating_duration_since(status_with_ranges_started_at));
    let saved_calibration = load_saved_calibration_for_channel(client, channel);
    let initial_status_started_at = Instant::now();
    let initial_status = ptz::get_ptz_cur_pos(client, channel)?;
    timeout_rtt_estimator
        .observe(Instant::now().saturating_duration_since(initial_status_started_at));
    let initial = map_status_to_raw_position(&initial_status)?;

    let (pan_nominal_min_count, pan_nominal_max_count) = axis_nominal_bounds(
        status_with_ranges
            .as_ref()
            .and_then(|status| status.pan_range.as_ref()),
        saved_calibration.as_ref().map(|stored| {
            (
                stored.calibration.pan_min_count,
                stored.calibration.pan_max_count,
            )
        }),
        DEFAULT_PAN_MIN_COUNT,
        DEFAULT_PAN_MAX_COUNT,
    );
    let (tilt_nominal_min_count, tilt_nominal_max_count) = axis_nominal_bounds(
        status_with_ranges
            .as_ref()
            .and_then(|status| status.tilt_range.as_ref()),
        saved_calibration.as_ref().map(|stored| {
            (
                stored.calibration.tilt_min_count,
                stored.calibration.tilt_max_count,
            )
        }),
        DEFAULT_TILT_MIN_COUNT,
        DEFAULT_TILT_MAX_COUNT,
    );
    let (pan_min_count, pan_max_count) = axis_count_bounds(
        status_with_ranges
            .as_ref()
            .and_then(|status| status.pan_range.as_ref()),
        saved_calibration.as_ref().map(|stored| {
            (
                stored.calibration.pan_min_count,
                stored.calibration.pan_max_count,
            )
        }),
        initial.pan_count,
        DEFAULT_PAN_MIN_COUNT,
        DEFAULT_PAN_MAX_COUNT,
    );
    let (tilt_min_count, tilt_max_count) = axis_count_bounds(
        status_with_ranges
            .as_ref()
            .and_then(|status| status.tilt_range.as_ref()),
        saved_calibration.as_ref().map(|stored| {
            (
                stored.calibration.tilt_min_count,
                stored.calibration.tilt_max_count,
            )
        }),
        initial.tilt_count,
        DEFAULT_TILT_MIN_COUNT,
        DEFAULT_TILT_MAX_COUNT,
    );

    let pan_span = (pan_max_count - pan_min_count).abs().max(1.0);
    let tilt_span = (tilt_max_count - tilt_min_count).abs().max(1.0);
    let pan_nominal_span = (pan_nominal_max_count - pan_nominal_min_count)
        .abs()
        .max(1.0);
    let tilt_nominal_span = (tilt_nominal_max_count - tilt_nominal_min_count)
        .abs()
        .max(1.0);
    let deadband_hints = load_axis_deadband_hints(saved_calibration.as_ref());
    let pan_success_tolerance = calibrated_success_tolerance(
        axis_one_percent_threshold(pan_nominal_span),
        deadband_hints.pan_count,
        tolerance_count as f64,
    );
    let tilt_success_tolerance = calibrated_success_tolerance(
        axis_one_percent_threshold(tilt_nominal_span),
        deadband_hints.tilt_count,
        tolerance_count as f64,
    );
    let (strict_pan_tolerance, strict_tilt_tolerance) = strict_success_tolerances();
    let pan_model = saved_calibration
        .as_ref()
        .map(|stored| sanitize_model_params(stored.calibration.pan_model, pan_span))
        .unwrap_or_else(|| model_for_span(pan_span));
    let tilt_model = saved_calibration
        .as_ref()
        .map(|stored| sanitize_model_params(stored.calibration.tilt_model, tilt_span))
        .unwrap_or_else(|| model_for_span(tilt_span));
    let pan_ekf_config = ekf_config(pan_min_count, pan_max_count);
    let tilt_ekf_config = ekf_config(tilt_min_count, tilt_max_count);

    let mut pan_filter = AxisEkf::new(pan_ekf_config, pan_model, initial.pan_count as f64);
    let mut tilt_filter = AxisEkf::new(tilt_ekf_config, tilt_model, initial.tilt_count as f64);
    let mut pan_gain_tracker = AxisOnlineGainTracker::seeded(pan_model.beta);
    let mut tilt_gain_tracker = AxisOnlineGainTracker::seeded(tilt_model.beta);
    let mut pan_pulse_lut = AxisPulseLut::seeded(pan_model.beta);
    let mut tilt_pulse_lut = AxisPulseLut::seeded(tilt_model.beta);
    let mut pending_pulse_observation = None;
    let mut position_settling = PositionSettlingTracker::new();
    let mut last_pan_u = 0.0;
    let mut last_tilt_u = 0.0;
    let mut loop_updates = 0usize;
    let mut stable_steps = 0usize;
    let mut best_observed: Option<BestObservedState> = None;
    let mut best_age_steps = 0usize;
    let mut command_trace: Vec<String> = Vec::new();
    let mut last_update_at = Instant::now();
    let mut tie_break_pan = true;
    let mut failure_modes = FailureModeCounters::default();
    let mut edge_push_lockout = EdgePushLockoutState::default();
    let mut edge_lockout_blocks = 0usize;
    let mut stale_status_streak = 0usize;
    let mut pan_reversals = 0usize;
    let mut tilt_reversals = 0usize;
    let mut strict_completion_grace_steps = 0usize;
    let mut close_axis_no_progress_steps = 0usize;
    let mut previous_close_error_max: Option<f64> = None;
    let mut prev_pan_error_measured: Option<f64> = None;
    let mut prev_tilt_error_measured: Option<f64> = None;
    let mut model_mismatch_cooldown_steps = 0usize;
    let started = Instant::now();
    let onvif_options = ptz_transport::get_onvif_configuration_options(client, channel)
        .ok()
        .flatten();
    let supports_relative_move = ptz_transport::supports_relative_move_for_channel(client, channel)
        .ok()
        .unwrap_or_else(ptz_transport::supports_relative_move);
    let backend_completion_min_age_ms = if onvif_options
        .as_ref()
        .is_some_and(|options| options.has_timeout_range)
    {
        BACKEND_COMPLETION_MIN_AGE_MS
    } else {
        BACKEND_COMPLETION_MIN_AGE_MS + 60
    };
    let min_updates = if timeout_ms >= 3_000 {
        MIN_ADAPTIVE_UPDATES
    } else {
        1
    };
    let mut previous_pan_measure = Some(initial.pan_count as f64);
    let mut previous_tilt_measure = Some(initial.tilt_count as f64);
    let mut dual_axis_interleave = DualAxisInterleaveState::default();

    if let Some(stored) = load_stored_ekf_state(state_path, state_key, channel)? {
        if let Some(restored_pan) =
            AxisEkf::from_snapshot(pan_ekf_config, pan_model, stored.pan.to_snapshot())
        {
            pan_filter = restored_pan;
        }
        if let Some(restored_tilt) =
            AxisEkf::from_snapshot(tilt_ekf_config, tilt_model, stored.tilt.to_snapshot())
        {
            tilt_filter = restored_tilt;
        }
        last_pan_u = stored.last_pan_u.clamp(-1.0, 1.0);
        last_tilt_u = stored.last_tilt_u.clamp(-1.0, 1.0);
        pan_gain_tracker = AxisOnlineGainTracker::from_seed_and_betas(
            pan_model.beta,
            stored.pan_positive_beta,
            stored.pan_negative_beta,
        );
        tilt_gain_tracker = AxisOnlineGainTracker::from_seed_and_betas(
            tilt_model.beta,
            stored.tilt_positive_beta,
            stored.tilt_negative_beta,
        );
        pan_pulse_lut = AxisPulseLut::from_seed_and_rates(
            pan_model.beta,
            stored.pan_positive_counts_per_ms,
            stored.pan_negative_counts_per_ms,
            stored.pan_positive_edge_counts_per_ms,
            stored.pan_negative_edge_counts_per_ms,
        );
        tilt_pulse_lut = AxisPulseLut::from_seed_and_rates(
            tilt_model.beta,
            stored.tilt_positive_counts_per_ms,
            stored.tilt_negative_counts_per_ms,
            stored.tilt_positive_edge_counts_per_ms,
            stored.tilt_negative_edge_counts_per_ms,
        );
    }

    loop {
        let loop_started_at = Instant::now();
        let status_request_started_at = Instant::now();
        let status = ptz::get_ptz_cur_pos(client, channel)?;
        timeout_rtt_estimator
            .observe(Instant::now().saturating_duration_since(status_request_started_at));
        let current = map_status_to_raw_position(&status)?;
        let pan_measure = current.pan_count as f64;
        let tilt_measure = current.tilt_count as f64;
        let now = Instant::now();
        let measured_dt_sec = now.saturating_duration_since(last_update_at).as_secs_f64();
        let effective_dt_sec = if measured_dt_sec.is_finite() && measured_dt_sec >= 1e-3 {
            measured_dt_sec
        } else {
            EKF_TS_SEC
        };
        last_update_at = now;
        let pan_estimate = pan_filter.update_with_dt(last_pan_u, pan_measure, effective_dt_sec);
        let tilt_estimate = tilt_filter.update_with_dt(last_tilt_u, tilt_measure, effective_dt_sec);
        let estimated_pan = pan_estimate.state.position + pan_estimate.state.bias;
        let estimated_tilt = tilt_estimate.state.position + tilt_estimate.state.bias;

        let pan_error_measured = target_pan_count as f64 - pan_measure;
        let tilt_error_measured = target_tilt_count as f64 - tilt_measure;
        let measured_error_norm =
            normalized_vector_error(pan_error_measured, tilt_error_measured, pan_span, tilt_span);
        let pan_error_estimated = target_pan_count as f64 - estimated_pan;
        let tilt_error_estimated = target_tilt_count as f64 - estimated_tilt;
        let pan_reversal_now = reversal_detected(
            prev_pan_error_measured,
            pan_error_measured,
            tolerance_count as f64,
            pan_success_tolerance,
        );
        let tilt_reversal_now = reversal_detected(
            prev_tilt_error_measured,
            tilt_error_measured,
            tolerance_count as f64,
            tilt_success_tolerance,
        );
        update_reversal_counter(
            &mut pan_reversals,
            &mut prev_pan_error_measured,
            pan_error_measured,
            tolerance_count as f64,
            pan_success_tolerance,
        );
        update_reversal_counter(
            &mut tilt_reversals,
            &mut prev_tilt_error_measured,
            tilt_error_measured,
            tolerance_count as f64,
            tilt_success_tolerance,
        );
        let pan_activation_tolerance = adaptive_axis_tolerance(
            axis_control_activation_tolerance(tolerance_count as f64, pan_success_tolerance, 0.30),
            pan_reversals,
            scale_directional_deadband(
                deadband_hints.pan_count,
                pan_measure,
                pan_min_count,
                pan_max_count,
            ),
        );
        let tilt_activation_tolerance = adaptive_axis_tolerance(
            axis_control_activation_tolerance(tolerance_count as f64, tilt_success_tolerance, 0.45),
            tilt_reversals,
            scale_directional_deadband(
                deadband_hints.tilt_count,
                tilt_measure,
                tilt_min_count,
                tilt_max_count,
            ),
        );
        let pan_command_tolerance =
            command_activation_tolerance(pan_activation_tolerance, strict_pan_tolerance);
        let tilt_command_tolerance =
            command_activation_tolerance(tilt_activation_tolerance, strict_tilt_tolerance);

        let prefer_measured_control = model_mismatch_cooldown_steps > 0;
        let pan_error_control = if prefer_measured_control {
            pan_error_measured
        } else {
            select_control_error(pan_error_estimated, pan_error_measured, tolerance_count)
        };
        let tilt_error_control = if prefer_measured_control {
            tilt_error_measured
        } else {
            select_control_error(tilt_error_estimated, tilt_error_measured, tolerance_count)
        };
        let pan_observed_delta = previous_pan_measure.map(|previous| pan_measure - previous);
        let tilt_observed_delta = previous_tilt_measure.map(|previous| tilt_measure - previous);
        previous_pan_measure = Some(pan_measure);
        previous_tilt_measure = Some(tilt_measure);
        let pan_predicted_delta = pan_gain_tracker.predicted_delta(last_pan_u);
        let tilt_predicted_delta = tilt_gain_tracker.predicted_delta(last_tilt_u);
        let pan_model_mismatch_now =
            model_mismatch_detected(pan_predicted_delta, pan_observed_delta);
        let tilt_model_mismatch_now =
            model_mismatch_detected(tilt_predicted_delta, tilt_observed_delta);
        let model_mismatch_now = pan_model_mismatch_now || tilt_model_mismatch_now;
        if model_mismatch_now {
            failure_modes.model_mismatch_hits = failure_modes.model_mismatch_hits.saturating_add(1);
            model_mismatch_cooldown_steps = MODEL_MISMATCH_RECOVERY_STEPS;
            dual_axis_interleave = DualAxisInterleaveState::default();
        }
        pan_gain_tracker.observe(last_pan_u, pan_observed_delta, pan_span);
        tilt_gain_tracker.observe(last_tilt_u, tilt_observed_delta, tilt_span);
        let stale_now = stale_status_detected(
            last_pan_u,
            last_tilt_u,
            pan_observed_delta,
            tilt_observed_delta,
            &mut stale_status_streak,
        );
        if stale_now {
            failure_modes.stale_status_hits = failure_modes.stale_status_hits.saturating_add(1);
        }
        let pan_axis_stale = axis_stale_detected(last_pan_u, pan_observed_delta);
        let tilt_axis_stale = axis_stale_detected(last_tilt_u, tilt_observed_delta);
        let pan_lut_sample_reliable = !pan_model_mismatch_now && !pan_axis_stale;
        let tilt_lut_sample_reliable = !tilt_model_mismatch_now && !tilt_axis_stale;
        let pan_noise_hint = measurement_noise_hint_scale(
            pan_model_mismatch_now,
            pan_axis_stale,
            pan_reversals,
            pan_error_measured,
            pan_success_tolerance,
        );
        let tilt_noise_hint = measurement_noise_hint_scale(
            tilt_model_mismatch_now,
            tilt_axis_stale,
            tilt_reversals,
            tilt_error_measured,
            tilt_success_tolerance,
        );
        pan_filter.apply_measurement_noise_hint(pan_noise_hint);
        tilt_filter.apply_measurement_noise_hint(tilt_noise_hint);
        apply_pending_pulse_observation(
            &mut pending_pulse_observation,
            pan_observed_delta,
            tilt_observed_delta,
            pan_lut_sample_reliable,
            tilt_lut_sample_reliable,
            &mut pan_pulse_lut,
            &mut tilt_pulse_lut,
        );
        let stable_threshold_count = position_stable_threshold_count(
            tolerance_count as f64,
            deadband_hints.pan_count,
            deadband_hints.tilt_count,
        );
        position_settling.observe(
            pan_observed_delta,
            tilt_observed_delta,
            stable_threshold_count,
        );
        let fine_phase_candidate = supports_relative_move
            && pan_error_measured.abs().max(tilt_error_measured.abs())
                <= FINE_PHASE_ENTRY_ERROR_COUNT
            && model_mismatch_cooldown_steps == 0;
        let pulse_lut_candidate = !supports_relative_move
            && pan_error_measured.abs().max(tilt_error_measured.abs())
                <= PULSE_LUT_ENTRY_ERROR_COUNT;
        let near_target_speed1_mode = !supports_relative_move
            && pan_error_measured.abs().max(tilt_error_measured.abs())
                <= NEAR_TARGET_SPEED1_ENTRY_ERROR_COUNT;

        let pan_guard_deadband = scale_directional_deadband(
            deadband_hints.pan_for_error(pan_error_control),
            pan_measure,
            pan_min_count,
            pan_max_count,
        );
        let tilt_guard_deadband = scale_directional_deadband(
            deadband_hints.tilt_for_error(tilt_error_control),
            tilt_measure,
            tilt_min_count,
            tilt_max_count,
        );
        let guarded_pan_error = apply_reversal_guard(
            pan_error_control,
            last_pan_u,
            pan_activation_tolerance,
            pan_guard_deadband,
        );
        let guarded_tilt_error = apply_reversal_guard(
            tilt_error_control,
            last_tilt_u,
            tilt_activation_tolerance,
            tilt_guard_deadband,
        );
        let guarded_tilt_error = apply_tilt_backlash_compensation(
            guarded_tilt_error,
            tilt_error_measured,
            tilt_guard_deadband,
            tilt_reversal_now,
        );
        let pan_command_error = if fine_phase_candidate {
            apply_fine_phase_feedforward(
                guarded_pan_error,
                pan_gain_tracker.beta_for_u(last_pan_u),
                last_pan_u,
                pan_observed_delta,
                pan_activation_tolerance,
            )
        } else {
            guarded_pan_error
        };
        let tilt_command_error = if fine_phase_candidate {
            apply_fine_phase_feedforward(
                guarded_tilt_error,
                tilt_gain_tracker.beta_for_u(last_tilt_u),
                last_tilt_u,
                tilt_observed_delta,
                tilt_activation_tolerance,
            )
        } else {
            guarded_tilt_error
        };
        let pan_command_error = enforce_residual_command_activity(
            pan_command_error,
            pan_error_measured,
            pan_command_tolerance,
            strict_pan_tolerance,
        );
        let tilt_command_error = enforce_residual_command_activity(
            tilt_command_error,
            tilt_error_measured,
            tilt_command_tolerance,
            strict_tilt_tolerance,
        );
        let command_error_norm =
            normalized_vector_error(pan_command_error, tilt_command_error, pan_span, tilt_span);
        let edge_context = EdgePushContext {
            pan_measure,
            pan_min_count,
            pan_max_count,
            pan_span,
            pan_error_measured,
            pan_success_tolerance,
            tilt_measure,
            tilt_min_count,
            tilt_max_count,
            tilt_error_measured,
            tilt_success_tolerance,
        };

        if consider_best(
            &mut best_observed,
            current.pan_count,
            current.tilt_count,
            target_pan_count,
            target_tilt_count,
        ) {
            best_age_steps = 0;
        } else {
            best_age_steps = best_age_steps.saturating_add(1);
        }
        loop_updates += 1;

        let within_strict_tolerance = pan_error_measured.abs() <= strict_pan_tolerance
            && tilt_error_measured.abs() <= strict_tilt_tolerance;
        if within_strict_tolerance {
            strict_completion_grace_steps = strict_completion_grace_steps.saturating_add(1);
        } else {
            strict_completion_grace_steps = 0;
        }
        let required_stable_steps =
            required_stable_steps_for_oscillation(pan_reversals, tilt_reversals);
        let backend_motion_hint = if force_cgi_pulse_transport {
            None
        } else {
            ptz_transport::motion_status_hint(client, channel)
        };
        let backend_completion_ready = if let Some(hint) = backend_motion_hint {
            completion_gate_allows_success(
                hint.moving,
                hint.move_age_ms,
                CompletionGateCapabilities::from_hint(hint.moving, hint.move_age_ms),
                backend_completion_min_age_ms,
                position_settling.stable_steps(),
                BACKEND_POSITION_STABLE_REQUIRED_STEPS,
                BACKEND_POSITION_STABLE_REQUIRED_STEPS_FALLBACK,
            )
        } else {
            position_settling.stable_steps() >= BACKEND_POSITION_STABLE_REQUIRED_STEPS
        };
        let backend_completion_with_grace = backend_completion_ready
            || (within_strict_tolerance
                && strict_completion_grace_steps >= STRICT_COMPLETION_GRACE_STEPS
                && position_settling.stable_steps() >= 1);
        if within_strict_tolerance && loop_updates >= min_updates && backend_completion_with_grace {
            stable_steps += 1;
        } else {
            stable_steps = 0;
        }

        if within_strict_tolerance && stable_steps >= required_stable_steps {
            save_stored_ekf_state(
                state_path,
                state_key,
                channel,
                &pan_filter,
                &tilt_filter,
                OnlineLearningState {
                    pan_gain_tracker: &pan_gain_tracker,
                    tilt_gain_tracker: &tilt_gain_tracker,
                    pan_pulse_lut: &pan_pulse_lut,
                    tilt_pulse_lut: &tilt_pulse_lut,
                    last_pan_u: 0.0,
                    last_tilt_u: 0.0,
                },
            )?;
            return Ok(current);
        }

        if let Some(best) = best_observed {
            let stagnation_ready =
                success_latch_stagnation_ready(best_age_steps, backend_motion_hint);
            let best_within_success_tol =
                best_within_success_tolerance(best, strict_pan_tolerance, strict_tilt_tolerance);
            let near_miss_latch_eligible = stagnation_near_miss_latch_eligible(
                best,
                strict_pan_tolerance,
                strict_tilt_tolerance,
                stagnation_ready,
                backend_motion_hint,
            );
            if (stagnation_ready && best_within_success_tol) || near_miss_latch_eligible {
                let success_position = PtzRawPosition {
                    channel: current.channel,
                    pan_count: best.pan_count,
                    tilt_count: best.tilt_count,
                    zoom_count: current.zoom_count,
                    focus_count: current.focus_count,
                };
                save_stored_ekf_state(
                    state_path,
                    state_key,
                    channel,
                    &pan_filter,
                    &tilt_filter,
                    OnlineLearningState {
                        pan_gain_tracker: &pan_gain_tracker,
                        tilt_gain_tracker: &tilt_gain_tracker,
                        pan_pulse_lut: &pan_pulse_lut,
                        tilt_pulse_lut: &tilt_pulse_lut,
                        last_pan_u: 0.0,
                        last_tilt_u: 0.0,
                    },
                )?;
                return Ok(success_position);
            }
        }

        let timeout_budget = adaptive_timeout_budget(timeout_ms, timeout_rtt_estimator);
        let adaptive_deadline = adaptive_timeout_deadline(deadline, timeout_budget);
        if Instant::now() >= adaptive_deadline {
            let best = best_observed.unwrap_or(BestObservedState {
                pan_count: current.pan_count,
                tilt_count: current.tilt_count,
                pan_abs_error: pan_error_measured.abs().round() as i64,
                tilt_abs_error: tilt_error_measured.abs().round() as i64,
            });
            let best_within_success_tol =
                best_within_success_tolerance(best, strict_pan_tolerance, strict_tilt_tolerance);
            let stagnation_ready =
                success_latch_stagnation_ready(best_age_steps, backend_motion_hint);
            let near_miss_latch_eligible = stagnation_near_miss_latch_eligible(
                best,
                strict_pan_tolerance,
                strict_tilt_tolerance,
                stagnation_ready,
                backend_motion_hint,
            );
            let latch_eligible = timeout_latch_eligible(
                position_settling.stable_steps(),
                measured_error_norm,
                best_within_success_tol,
                stagnation_ready,
                near_miss_latch_eligible,
                backend_motion_hint,
            );
            if (within_strict_tolerance && backend_completion_with_grace) || latch_eligible {
                let success_position = if within_strict_tolerance && backend_completion_with_grace {
                    current.clone()
                } else {
                    PtzRawPosition {
                        channel: current.channel,
                        pan_count: best.pan_count,
                        tilt_count: best.tilt_count,
                        zoom_count: current.zoom_count,
                        focus_count: current.focus_count,
                    }
                };
                save_stored_ekf_state(
                    state_path,
                    state_key,
                    channel,
                    &pan_filter,
                    &tilt_filter,
                    OnlineLearningState {
                        pan_gain_tracker: &pan_gain_tracker,
                        tilt_gain_tracker: &tilt_gain_tracker,
                        pan_pulse_lut: &pan_pulse_lut,
                        tilt_pulse_lut: &tilt_pulse_lut,
                        last_pan_u: 0.0,
                        last_tilt_u: 0.0,
                    },
                )?;
                return Ok(success_position);
            }
            let persist_error = save_stored_ekf_state(
                state_path,
                state_key,
                channel,
                &pan_filter,
                &tilt_filter,
                OnlineLearningState {
                    pan_gain_tracker: &pan_gain_tracker,
                    tilt_gain_tracker: &tilt_gain_tracker,
                    pan_pulse_lut: &pan_pulse_lut,
                    tilt_pulse_lut: &tilt_pulse_lut,
                    last_pan_u,
                    last_tilt_u,
                },
            )
            .err();
            let persist_note = persist_error
                .as_ref()
                .map(|error| format!("; persist_error={}", error.message))
                .unwrap_or_default();
            let timeout_blocker = timeout_blocker_label(
                within_strict_tolerance,
                backend_completion_ready,
                best_within_success_tol,
                latch_eligible,
            );
            let dominant_failure_mode = dominant_failure_mode_label(failure_modes);
            let pan_consistency = pan_filter.consistency();
            let tilt_consistency = tilt_filter.consistency();
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "set_absolute_raw timeout after {}ms on channel {channel}: target=({},{}) current=({},{}) measured_error=({:.1},{:.1}) measured_error_norm={:.5} estimated_error=({:.1},{:.1}) control_error=({:.1},{:.1}) command_error=({:.1},{:.1}) command_error_norm={:.5} tolerance={} control_tolerance=({:.1},{:.1}) command_tolerance=({:.1},{:.1}) success_tolerance=({:.1},{:.1}) reversals=({},{}) updates={} stable_steps={} failure_modes=(edge:{},model:{},axis_swap:{},stale:{}) edge_lockout_blocks={} adaptive_timeout=(base_ms:{},raw_slack_ms:{},slack_cap_ms:{},applied_slack_ms:{},effective_ms:{},samples:{},srtt_ms:{:.1},rttvar_ms:{:.1},last_rtt_ms:{:.1}) timeout_blocker={} dominant_failure_mode={} best_within_success_tol={} latch_eligible={} best_age_steps={} online_beta=(pan+:{:.1},pan-:{:.1},tilt+:{:.1},tilt-:{:.1}) ekf_consistency=(pan_nis:{:.2}/{:.2},tilt_nis:{:.2}/{:.2},pan_r:{:.2},tilt_r:{:.2},pan_resvar:{:.2},tilt_resvar:{:.2}) last_dt_sec={:.3} backend_hint={} best=({},{}) best_error=({},{}) trace=[{}]{}",
                    timeout_budget.effective_timeout_ms,
                    target_pan_count,
                    target_tilt_count,
                    current.pan_count,
                    current.tilt_count,
                    pan_error_measured,
                    tilt_error_measured,
                    measured_error_norm,
                    pan_error_estimated,
                    tilt_error_estimated,
                    pan_error_control,
                    tilt_error_control,
                    pan_command_error,
                    tilt_command_error,
                    command_error_norm,
                    tolerance_count,
                    pan_activation_tolerance,
                    tilt_activation_tolerance,
                    pan_command_tolerance,
                    tilt_command_tolerance,
                    pan_success_tolerance,
                    tilt_success_tolerance,
                    pan_reversals,
                    tilt_reversals,
                    loop_updates,
                    stable_steps,
                    failure_modes.edge_saturation_hits,
                    failure_modes.model_mismatch_hits,
                    failure_modes.axis_swap_lag_hits,
                    failure_modes.stale_status_hits,
                    edge_lockout_blocks,
                    timeout_budget.base_timeout_ms,
                    timeout_budget.raw_slack_ms,
                    timeout_budget.slack_cap_ms,
                    timeout_budget.applied_slack_ms,
                    timeout_budget.effective_timeout_ms,
                    timeout_budget.sample_count,
                    timeout_budget.srtt_ms,
                    timeout_budget.rttvar_ms,
                    timeout_budget.last_sample_ms,
                    timeout_blocker,
                    dominant_failure_mode,
                    best_within_success_tol,
                    latch_eligible,
                    best_age_steps,
                    pan_gain_tracker.positive_beta(),
                    pan_gain_tracker.negative_beta(),
                    tilt_gain_tracker.positive_beta(),
                    tilt_gain_tracker.negative_beta(),
                    pan_consistency.last_nis,
                    pan_consistency.ewma_nis,
                    tilt_consistency.last_nis,
                    tilt_consistency.ewma_nis,
                    pan_consistency.adaptive_r,
                    tilt_consistency.adaptive_r,
                    pan_consistency.residual_variance_proxy,
                    tilt_consistency.residual_variance_proxy,
                    effective_dt_sec,
                    format_transport_hint(backend_motion_hint),
                    best.pan_count,
                    best.tilt_count,
                    best.pan_abs_error,
                    best.tilt_abs_error,
                    command_trace.join(" | "),
                    persist_note,
                ),
            ));
        }

        let dual_axis_active = pan_command_error.abs() > pan_command_tolerance
            && tilt_command_error.abs() > tilt_command_tolerance;
        let dual_axis_close = dual_axis_active
            && pan_command_error.abs() <= TIE_BREAK_CLOSE_ERROR_COUNT
            && tilt_command_error.abs() <= TIE_BREAK_CLOSE_ERROR_COUNT;
        if dual_axis_close {
            let close_error_max = pan_command_error.abs().max(tilt_command_error.abs());
            let progressed = previous_close_error_max.is_some_and(|previous| {
                close_error_max + DUAL_AXIS_CLOSE_PROGRESS_EPS_COUNT < previous
            });
            if progressed {
                close_axis_no_progress_steps = 0;
            } else {
                close_axis_no_progress_steps = close_axis_no_progress_steps.saturating_add(1);
            }
            previous_close_error_max = Some(close_error_max);
        } else {
            close_axis_no_progress_steps = 0;
            previous_close_error_max = None;
        }
        let mut step_error_abs = pan_command_error.abs().max(tilt_command_error.abs());
        let mut relative_command_applied = false;

        if fine_phase_candidate {
            let pan_relative_delta =
                relative_delta_from_error(pan_command_error, pan_command_tolerance);
            let tilt_relative_delta =
                relative_delta_from_error(tilt_command_error, tilt_command_tolerance);
            if pan_relative_delta != 0 || tilt_relative_delta != 0 {
                relative_command_applied = ptz_transport::move_relative_ptz(
                    client,
                    channel,
                    pan_relative_delta,
                    tilt_relative_delta,
                )?;
                if relative_command_applied {
                    let elapsed_ms = Instant::now()
                        .saturating_duration_since(started)
                        .as_millis();
                    command_trace.push(format!(
                        "t={}ms:rel=({},{}) e=({:.1},{:.1})",
                        elapsed_ms,
                        pan_relative_delta,
                        tilt_relative_delta,
                        pan_command_error,
                        tilt_command_error
                    ));
                    if command_trace.len() > 24 {
                        command_trace.remove(0);
                    }
                    let (pan_u, tilt_u) = control_components_from_relative_step(
                        pan_relative_delta,
                        tilt_relative_delta,
                    );
                    last_pan_u = pan_u;
                    last_tilt_u = tilt_u;
                    pending_pulse_observation = None;
                    if pan_relative_delta != 0 && tilt_relative_delta != 0 {
                        tie_break_pan = !tie_break_pan;
                    }
                }
            }
        }

        if !relative_command_applied {
            let strict_axis_focus = strict_axis_focus_command(
                pan_error_measured,
                tilt_error_measured,
                strict_pan_tolerance,
                strict_tilt_tolerance,
            );
            if strict_axis_focus.is_some() {
                dual_axis_interleave = DualAxisInterleaveState::default();
            }
            let forced_secondary = if strict_axis_focus.is_some() {
                None
            } else {
                forced_secondary_axis_command(
                    &mut dual_axis_interleave,
                    pan_command_error,
                    tilt_command_error,
                    pan_command_tolerance,
                    tilt_command_tolerance,
                    pan_span,
                    tilt_span,
                    pan_success_tolerance,
                    tilt_success_tolerance,
                    strict_pan_tolerance,
                    strict_tilt_tolerance,
                )
            };
            let selected_command = strict_axis_focus.or(forced_secondary).or_else(|| {
                command_from_errors(
                    pan_command_error,
                    tilt_command_error,
                    pan_command_tolerance,
                    tilt_command_tolerance,
                    tie_break_pan,
                    pan_span,
                    tilt_span,
                    pan_success_tolerance,
                    tilt_success_tolerance,
                )
            });
            let (selected_command, edge_lockout_blocked) = select_command_with_edge_push_lockout(
                &mut edge_push_lockout,
                selected_command,
                pan_command_error,
                tilt_command_error,
                pan_command_tolerance,
                tilt_command_tolerance,
                edge_context,
            );
            if edge_lockout_blocked {
                edge_lockout_blocks = edge_lockout_blocks.saturating_add(1);
            }
            match selected_command {
                Some((direction, command_error_abs)) => {
                    step_error_abs = command_error_abs;
                    if axis_swap_lag_detected(
                        pan_command_error,
                        tilt_command_error,
                        direction,
                        pan_span,
                        tilt_span,
                        pan_success_tolerance,
                        tilt_success_tolerance,
                    ) {
                        failure_modes.axis_swap_lag_hits =
                            failure_modes.axis_swap_lag_hits.saturating_add(1);
                    }
                    if edge_context.risky_push(direction) {
                        failure_modes.edge_saturation_hits =
                            failure_modes.edge_saturation_hits.saturating_add(1);
                    }
                    let speed = if near_target_speed1_mode {
                        1
                    } else {
                        speed_cap_for_error(command_error_abs).max(1)
                    };
                    let base_pulse_ms = if pulse_lut_candidate {
                        let lut_edge_band = pulse_lut_edge_band_for_command(
                            direction,
                            pan_measure,
                            pan_min_count,
                            pan_max_count,
                            tilt_measure,
                            tilt_min_count,
                            tilt_max_count,
                        );
                        pulse_ms_for_direction_with_lut(
                            direction,
                            pan_command_error,
                            tilt_command_error,
                            &pan_pulse_lut,
                            &tilt_pulse_lut,
                            command_error_abs,
                            lut_edge_band,
                        )
                    } else {
                        control_pulse_ms_for_error(command_error_abs)
                    };
                    let pulse_ms = if near_target_speed1_mode {
                        near_target_speed1_pulse_ms(base_pulse_ms, command_error_abs, direction)
                    } else {
                        base_pulse_ms
                    };
                    let (speed, pulse_ms) = clamp_tilt_edge_control(
                        direction,
                        speed,
                        pulse_ms,
                        tilt_measure,
                        tilt_min_count,
                        tilt_max_count,
                    );
                    let (speed, pulse_ms) = if oscillation_damping_active(
                        pan_reversals,
                        tilt_reversals,
                        pan_error_measured,
                        tilt_error_measured,
                        pan_success_tolerance,
                        tilt_success_tolerance,
                    ) {
                        (speed.min(1), pulse_ms.min(OSCILLATION_DAMPING_PULSE_MS_MAX))
                    } else {
                        (speed, pulse_ms)
                    };
                    let (speed, pulse_ms) = clamp_pan_reversal_micro_control(
                        direction,
                        speed,
                        pulse_ms,
                        pan_reversal_now,
                        pan_error_measured,
                    );
                    let (speed, pulse_ms) = clamp_tilt_reversal_micro_control(
                        direction,
                        speed,
                        pulse_ms,
                        tilt_reversal_now,
                        tilt_error_measured,
                        tilt_guard_deadband,
                    );
                    let pulse_ms = ensure_active_command_pulse_ms(
                        pulse_ms,
                        direction,
                        pan_command_error,
                        tilt_command_error,
                        strict_pan_tolerance,
                        strict_tilt_tolerance,
                    );
                    move_ptz_for_absolute(
                        client,
                        channel,
                        direction,
                        speed,
                        pulse_ms,
                        force_cgi_pulse_transport,
                    )?;
                    let lut_edge_band = pulse_lut_edge_band_for_command(
                        direction,
                        pan_measure,
                        pan_min_count,
                        pan_max_count,
                        tilt_measure,
                        tilt_min_count,
                        tilt_max_count,
                    );
                    pending_pulse_observation = pending_pulse_observation_for_lut_command(
                        direction,
                        pulse_ms,
                        lut_edge_band,
                        pulse_lut_learning_allowed(
                            direction,
                            pulse_lut_candidate,
                            pulse_ms,
                            pan_reversal_now,
                            tilt_reversal_now,
                        ),
                    );
                    let elapsed_ms = Instant::now()
                        .saturating_duration_since(started)
                        .as_millis();
                    command_trace.push(format!(
                        "t={}ms:{:?}/s{}/{}ms e=({:.1},{:.1})",
                        elapsed_ms,
                        direction,
                        speed,
                        pulse_ms,
                        pan_command_error,
                        tilt_command_error
                    ));
                    if command_trace.len() > 24 {
                        command_trace.remove(0);
                    }
                    let (pan_u, tilt_u) =
                        control_components_from_command(direction, speed, pulse_ms);
                    last_pan_u = pan_u;
                    last_tilt_u = tilt_u;
                    if dual_axis_close
                        && close_axis_no_progress_steps >= DUAL_AXIS_CLOSE_NO_PROGRESS_TOGGLE_STEPS
                    {
                        tie_break_pan = !tie_break_pan;
                        close_axis_no_progress_steps = 0;
                    }
                }
                None => {
                    last_pan_u = 0.0;
                    last_tilt_u = 0.0;
                    pending_pulse_observation = None;
                }
            }
        }

        save_stored_ekf_state(
            state_path,
            state_key,
            channel,
            &pan_filter,
            &tilt_filter,
            OnlineLearningState {
                pan_gain_tracker: &pan_gain_tracker,
                tilt_gain_tracker: &tilt_gain_tracker,
                pan_pulse_lut: &pan_pulse_lut,
                tilt_pulse_lut: &tilt_pulse_lut,
                last_pan_u,
                last_tilt_u,
            },
        )?;

        if model_mismatch_cooldown_steps > 0 {
            model_mismatch_cooldown_steps = model_mismatch_cooldown_steps.saturating_sub(1);
        }
        let sleep_duration = remaining_control_step_sleep_duration(
            control_step_ms_for_error(step_error_abs),
            Instant::now().saturating_duration_since(loop_started_at),
        );
        if sleep_duration > Duration::ZERO {
            thread::sleep(sleep_duration);
        } else {
            thread::yield_now();
        }
    }
}

fn model_for_span(span: f64) -> AxisModelParams {
    AxisModelParams {
        alpha: MODEL_ALPHA_DEFAULT,
        beta: (span * MODEL_BETA_RATIO).clamp(MODEL_BETA_MIN, MODEL_BETA_MAX),
    }
}

fn sanitize_model_params(model: AxisModelParams, span: f64) -> AxisModelParams {
    let fallback = model_for_span(span);
    let alpha = if model.alpha.is_finite() {
        model.alpha.clamp(0.5, 0.999)
    } else {
        fallback.alpha
    };
    let beta = if model.beta.is_finite() {
        model.beta.clamp(MODEL_BETA_MIN, MODEL_BETA_MAX)
    } else {
        fallback.beta
    };

    AxisModelParams { alpha, beta }
}

impl AxisOnlineGainTracker {
    fn seeded(model_beta: f64) -> Self {
        let seeded = if model_beta.is_finite() {
            model_beta.clamp(MODEL_BETA_MIN, MODEL_BETA_MAX)
        } else {
            MODEL_BETA_MIN
        };
        Self {
            positive_beta: seeded,
            negative_beta: seeded,
        }
    }

    fn from_seed_and_betas(
        model_beta: f64,
        positive_beta: Option<f64>,
        negative_beta: Option<f64>,
    ) -> Self {
        let mut tracker = Self::seeded(model_beta);
        if let Some(value) = sanitize_stored_beta(positive_beta) {
            tracker.positive_beta = value;
        }
        if let Some(value) = sanitize_stored_beta(negative_beta) {
            tracker.negative_beta = value;
        }
        tracker
    }

    fn beta_for_u(self, control_u: f64) -> f64 {
        if control_u.is_sign_negative() {
            self.negative_beta
        } else {
            self.positive_beta
        }
    }

    fn predicted_delta(self, control_u: f64) -> f64 {
        if !control_u.is_finite() {
            return 0.0;
        }
        let u = control_u.clamp(-1.0, 1.0);
        self.beta_for_u(u) * u
    }

    fn observe(&mut self, control_u: f64, observed_delta: Option<f64>, axis_span: f64) {
        if !control_u.is_finite() {
            return;
        }
        let Some(observed_delta) = observed_delta.filter(|value| value.is_finite()) else {
            return;
        };
        let u = control_u.clamp(-1.0, 1.0);
        let u_abs = u.abs();
        if u_abs < ONLINE_BETA_MIN_CONTROL_U {
            return;
        }
        if observed_delta.abs() < ONLINE_BETA_SAMPLE_MIN_COUNT {
            return;
        }
        if observed_delta.signum() != u.signum() {
            return;
        }

        let mut sample_beta = (observed_delta.abs() / u_abs).clamp(MODEL_BETA_MIN, MODEL_BETA_MAX);
        let sample_upper =
            (axis_span * ONLINE_BETA_SAMPLE_SPAN_RATIO_MAX).clamp(MODEL_BETA_MIN, MODEL_BETA_MAX);
        if sample_beta > sample_upper {
            return;
        }
        sample_beta = sample_beta.min(sample_upper);

        let target_beta = if u.is_sign_negative() {
            &mut self.negative_beta
        } else {
            &mut self.positive_beta
        };
        let previous = *target_beta;
        let outlier_low = previous * ONLINE_BETA_OUTLIER_LOW_RATIO;
        let outlier_high = previous * ONLINE_BETA_OUTLIER_HIGH_RATIO;
        if sample_beta < outlier_low || sample_beta > outlier_high {
            return;
        }

        let alpha = ONLINE_BETA_EWMA_ALPHA.clamp(0.05, 0.95);
        *target_beta =
            ((1.0 - alpha) * previous + alpha * sample_beta).clamp(MODEL_BETA_MIN, MODEL_BETA_MAX);
    }

    fn positive_beta(self) -> f64 {
        self.positive_beta
    }

    fn negative_beta(self) -> f64 {
        self.negative_beta
    }
}

fn sanitize_stored_beta(value: Option<f64>) -> Option<f64> {
    let value = value?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(MODEL_BETA_MIN, MODEL_BETA_MAX))
}

fn strict_success_tolerances() -> (f64, f64) {
    let thresholds = runtime::ptz_strict_success_thresholds_from_env();
    (thresholds.pan_count, thresholds.tilt_count)
}

fn best_within_success_tolerance(
    best: BestObservedState,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
) -> bool {
    (best.pan_abs_error as f64) <= pan_success_tolerance
        && (best.tilt_abs_error as f64) <= tilt_success_tolerance
}

fn success_latch_ready(
    settling_steps: usize,
    measured_error_norm: f64,
    backend_hint: Option<ptz_transport::TransportMotionHint>,
) -> bool {
    let backend_is_moving = backend_hint.and_then(|hint| hint.moving).unwrap_or(false);
    if backend_is_moving {
        return false;
    }
    if settling_steps >= SUCCESS_LATCH_MIN_SETTLING_STEPS {
        return true;
    }
    measured_error_norm.is_finite() && measured_error_norm <= SUCCESS_LATCH_CURRENT_NORM_MAX
}

fn success_latch_stagnation_ready(
    best_age_steps: usize,
    backend_hint: Option<ptz_transport::TransportMotionHint>,
) -> bool {
    let backend_is_moving = backend_hint.and_then(|hint| hint.moving).unwrap_or(false);
    if backend_is_moving {
        return false;
    }
    best_age_steps >= SUCCESS_LATCH_STAGNATION_MIN_BEST_AGE_STEPS
}

fn best_stagnation_near_miss_eligible(
    best: BestObservedState,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
    margin_count: f64,
) -> bool {
    if !pan_success_tolerance.is_finite()
        || !tilt_success_tolerance.is_finite()
        || !margin_count.is_finite()
        || margin_count <= 0.0
    {
        return false;
    }
    let pan_excess = (best.pan_abs_error as f64 - pan_success_tolerance).max(0.0);
    let tilt_excess = (best.tilt_abs_error as f64 - tilt_success_tolerance).max(0.0);
    let over_axes = usize::from(pan_excess > 0.0) + usize::from(tilt_excess > 0.0);
    if over_axes != 1 {
        return false;
    }
    pan_excess.max(tilt_excess) <= margin_count
}

fn backend_hint_unavailable_or_unknown(
    backend_hint: Option<ptz_transport::TransportMotionHint>,
) -> bool {
    backend_hint.and_then(|hint| hint.moving).is_none()
}

fn stagnation_near_miss_latch_eligible(
    best: BestObservedState,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
    stagnation_ready: bool,
    backend_hint: Option<ptz_transport::TransportMotionHint>,
) -> bool {
    if !stagnation_ready || !backend_hint_unavailable_or_unknown(backend_hint) {
        return false;
    }
    best_stagnation_near_miss_eligible(
        best,
        pan_success_tolerance,
        tilt_success_tolerance,
        SUCCESS_LATCH_STAGNATION_NEAR_MISS_MARGIN_COUNT,
    )
}

fn timeout_latch_eligible(
    settling_steps: usize,
    measured_error_norm: f64,
    best_within_success_tol: bool,
    stagnation_ready: bool,
    near_miss_latch_eligible: bool,
    backend_hint: Option<ptz_transport::TransportMotionHint>,
) -> bool {
    if near_miss_latch_eligible {
        return true;
    }
    best_within_success_tol
        && (success_latch_ready(settling_steps, measured_error_norm, backend_hint)
            || stagnation_ready)
}

fn timeout_blocker_label(
    within_tolerance: bool,
    backend_completion_ready: bool,
    best_within_success_tol: bool,
    latch_eligible: bool,
) -> &'static str {
    if within_tolerance && !backend_completion_ready {
        return "completion_gate";
    }
    if best_within_success_tol && !latch_eligible {
        return "latch_gate";
    }
    if !within_tolerance {
        return "residual_error";
    }
    "unknown"
}

fn dominant_failure_mode_label(counters: FailureModeCounters) -> &'static str {
    let edge = counters.edge_saturation_hits;
    let model = counters.model_mismatch_hits;
    let axis_swap = counters.axis_swap_lag_hits;
    let stale = counters.stale_status_hits;
    let max = edge.max(model).max(axis_swap).max(stale);
    if max == 0 {
        return "none";
    }

    let leader_count = usize::from(edge == max)
        + usize::from(model == max)
        + usize::from(axis_swap == max)
        + usize::from(stale == max);
    if leader_count > 1 {
        return "tie";
    }
    if edge == max {
        "edge"
    } else if model == max {
        "model"
    } else if axis_swap == max {
        "axis_swap"
    } else {
        "stale"
    }
}

fn load_saved_calibration_for_channel(client: &Client, channel: u8) -> Option<StoredCalibration> {
    let device_info = device::get_dev_info(client).ok()?;
    let (stored, _) = load_saved_params_for_device(&device_info).ok().flatten()?;
    if stored.channel == channel {
        Some(stored)
    } else {
        None
    }
}

fn load_axis_deadband_hints(stored: Option<&StoredCalibration>) -> AxisDeadbandHints {
    let defaults = AxisDeadbandHints {
        pan_count: 0.0,
        tilt_count: 0.0,
        pan_positive_count: 0.0,
        pan_negative_count: 0.0,
        tilt_positive_count: 0.0,
        tilt_negative_count: 0.0,
    };

    let Some(stored) = stored else {
        return defaults;
    };

    let pan_positive_count = effective_deadband_hint(
        stored
            .calibration
            .pan_deadband_increase_count
            .unwrap_or(stored.calibration.pan_deadband_count),
    );
    let pan_negative_count = effective_deadband_hint(
        stored
            .calibration
            .pan_deadband_decrease_count
            .unwrap_or(stored.calibration.pan_deadband_count),
    );
    let tilt_positive_count = effective_deadband_hint(
        stored
            .calibration
            .tilt_deadband_increase_count
            .unwrap_or(stored.calibration.tilt_deadband_count),
    );
    let tilt_negative_count = effective_deadband_hint(
        stored
            .calibration
            .tilt_deadband_decrease_count
            .unwrap_or(stored.calibration.tilt_deadband_count),
    );

    AxisDeadbandHints {
        pan_count: pan_positive_count.max(pan_negative_count),
        tilt_count: tilt_positive_count.max(tilt_negative_count),
        pan_positive_count,
        pan_negative_count,
        tilt_positive_count,
        tilt_negative_count,
    }
}

fn effective_deadband_hint(deadband_count: i64) -> f64 {
    ((deadband_count.unsigned_abs().max(1)) as f64).clamp(0.0, CALIBRATION_DEADBAND_HINT_MAX_COUNT)
}

fn ekf_config(min_position: f64, max_position: f64) -> AxisEkfConfig {
    let mut config = AxisEkfConfig::with_default_noise(EKF_TS_SEC, min_position, max_position);
    let span = (max_position - min_position).abs().max(1.0);
    let velocity_limit = (span * EKF_VELOCITY_RATIO).clamp(
        EKF_MIN_VELOCITY_COUNT_PER_SEC,
        EKF_MAX_VELOCITY_COUNT_PER_SEC,
    );
    let bias_limit = (span * EKF_MAX_BIAS_RATIO).clamp(EKF_MIN_BIAS_COUNT, EKF_MAX_BIAS_COUNT);
    config.q_position =
        (span * EKF_Q_POSITION_SPAN_RATIO).clamp(EKF_MIN_Q_POSITION, EKF_MAX_Q_POSITION);
    config.q_velocity =
        (span * EKF_Q_VELOCITY_SPAN_RATIO).clamp(EKF_MIN_Q_VELOCITY, EKF_MAX_Q_VELOCITY);
    config.q_bias = (span * EKF_Q_BIAS_SPAN_RATIO).clamp(EKF_MIN_Q_BIAS, EKF_MAX_Q_BIAS);
    config.r_measurement =
        (span * EKF_R_MEASUREMENT_SPAN_RATIO).clamp(EKF_MIN_R_MEASUREMENT, EKF_MAX_R_MEASUREMENT);
    config.min_velocity = -velocity_limit;
    config.max_velocity = velocity_limit;
    config.min_bias = -bias_limit;
    config.max_bias = bias_limit;
    config
}

fn axis_count_bounds(
    range: Option<&NumericRange>,
    calibration_bounds: Option<(i64, i64)>,
    current_count: i64,
    fallback_min: f64,
    fallback_max: f64,
) -> (f64, f64) {
    let (mut min_count, mut max_count) =
        resolved_axis_bounds(range, calibration_bounds, fallback_min, fallback_max);

    let current = current_count as f64;
    min_count = min_count.min(current) - EKF_POSITION_MARGIN_COUNT;
    max_count = max_count.max(current) + EKF_POSITION_MARGIN_COUNT;
    if max_count <= min_count {
        return resolved_axis_bounds(range, calibration_bounds, fallback_min, fallback_max);
    }
    (min_count, max_count)
}

fn axis_nominal_bounds(
    range: Option<&NumericRange>,
    calibration_bounds: Option<(i64, i64)>,
    fallback_min: f64,
    fallback_max: f64,
) -> (f64, f64) {
    resolved_axis_bounds(range, calibration_bounds, fallback_min, fallback_max)
}

fn resolved_axis_bounds(
    range: Option<&NumericRange>,
    calibration_bounds: Option<(i64, i64)>,
    fallback_min: f64,
    fallback_max: f64,
) -> (f64, f64) {
    let mut min_count = fallback_min.min(fallback_max);
    let mut max_count = fallback_max.max(fallback_min);

    if let Some((calibration_min, calibration_max)) = calibration_bounds {
        let raw_min = calibration_min as f64;
        let raw_max = calibration_max as f64;
        if raw_min.is_finite() && raw_max.is_finite() {
            min_count = raw_min.min(raw_max);
            max_count = raw_min.max(raw_max);
        }
    } else if let Some(bounds) = range {
        let raw_min = bounds.min as f64;
        let raw_max = bounds.max as f64;
        if raw_min.is_finite() && raw_max.is_finite() {
            min_count = raw_min.min(raw_max);
            max_count = raw_min.max(raw_max);
        }
    }

    if max_count <= min_count {
        (
            fallback_min.min(fallback_max),
            fallback_max.max(fallback_min),
        )
    } else {
        (min_count, max_count)
    }
}

fn select_control_error(estimated_error: f64, measured_error: f64, tolerance_count: i64) -> f64 {
    if !estimated_error.is_finite() {
        return measured_error;
    }
    let tolerance = tolerance_count as f64;
    if measured_error.abs() <= tolerance {
        return measured_error;
    }
    if estimated_error.signum() != measured_error.signum() {
        return measured_error;
    }

    let disagreement = (estimated_error - measured_error).abs();
    let allowed = measured_error.abs().max(10.0) * 1.2;
    if disagreement > allowed {
        measured_error
    } else {
        estimated_error
    }
}

fn axis_percent_threshold(span: f64, ratio: f64) -> f64 {
    if !span.is_finite() || span <= f64::EPSILON {
        return 1.0;
    }
    let normalized_ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if normalized_ratio <= f64::EPSILON {
        return 1.0;
    }
    (span * normalized_ratio).floor().max(1.0)
}

fn axis_one_percent_threshold(span: f64) -> f64 {
    axis_percent_threshold(span, 0.01)
}

fn calibrated_success_tolerance(
    base_tolerance: f64,
    deadband_hint_count: f64,
    control_tolerance: f64,
) -> f64 {
    let base = if base_tolerance.is_finite() {
        base_tolerance.max(1.0)
    } else {
        1.0
    };
    if !deadband_hint_count.is_finite() || deadband_hint_count <= 0.0 {
        return base;
    }
    let margin = control_tolerance.max(0.0) * SUCCESS_TOLERANCE_DEADBAND_MARGIN_RATIO;
    let calibrated_floor = (deadband_hint_count + margin).min(SUCCESS_TOLERANCE_DEADBAND_MAX_COUNT);
    base.max(calibrated_floor)
}

fn axis_control_activation_tolerance(
    base_tolerance: f64,
    success_tolerance: f64,
    success_ratio: f64,
) -> f64 {
    let base = if base_tolerance.is_finite() {
        base_tolerance.max(1.0)
    } else {
        1.0
    };
    if !success_tolerance.is_finite() || success_tolerance <= 0.0 {
        return base;
    }
    base.max(success_tolerance * success_ratio.clamp(0.05, 1.0))
}

fn command_activation_tolerance(control_tolerance: f64, strict_tolerance: f64) -> f64 {
    let control = if control_tolerance.is_finite() {
        control_tolerance.max(1.0)
    } else {
        1.0
    };
    if strict_tolerance.is_finite() && strict_tolerance > f64::EPSILON {
        control.min(strict_tolerance.max(1.0))
    } else {
        control
    }
}

fn normalized_axis_error(abs_error: f64, span: f64) -> f64 {
    if !abs_error.is_finite() || !span.is_finite() || span <= f64::EPSILON {
        return abs_error.abs();
    }
    abs_error.abs() / span
}

fn axis_control_priority(abs_error: f64, span: f64, success_tolerance: f64) -> f64 {
    if !abs_error.is_finite() {
        return 0.0;
    }
    let span_norm = normalized_axis_error(abs_error, span);
    let success_norm = if success_tolerance.is_finite() && success_tolerance > f64::EPSILON {
        abs_error.abs() / success_tolerance
    } else {
        abs_error.abs()
    };
    span_norm.max(success_norm)
}

fn normalized_vector_error(pan_error: f64, tilt_error: f64, pan_span: f64, tilt_span: f64) -> f64 {
    let pan_component = normalized_axis_error(pan_error, pan_span);
    let tilt_component = normalized_axis_error(tilt_error, tilt_span);
    (pan_component.mul_add(pan_component, tilt_component * tilt_component)).sqrt()
}

fn model_mismatch_detected(predicted_delta: f64, observed_delta: Option<f64>) -> bool {
    if !predicted_delta.is_finite() {
        return false;
    }
    let Some(observed_delta) = observed_delta.filter(|value| value.is_finite()) else {
        return false;
    };
    let predicted_abs = predicted_delta.abs();
    if predicted_abs < FAILURE_DIAG_MODEL_MISMATCH_MIN_COUNT {
        return false;
    }
    let residual_abs = (predicted_delta - observed_delta).abs();
    let allowed = (predicted_abs * FAILURE_DIAG_MODEL_MISMATCH_RATIO)
        .max(FAILURE_DIAG_MODEL_MISMATCH_MIN_COUNT);
    residual_abs > allowed
}

fn stale_status_detected(
    last_pan_u: f64,
    last_tilt_u: f64,
    pan_observed_delta: Option<f64>,
    tilt_observed_delta: Option<f64>,
    streak: &mut usize,
) -> bool {
    let commanded_motion = last_pan_u.abs() > 0.05 || last_tilt_u.abs() > 0.05;
    if !commanded_motion {
        *streak = 0;
        return false;
    }

    let pan_stationary = pan_observed_delta
        .filter(|value| value.is_finite())
        .is_some_and(|value| value.abs() <= FAILURE_DIAG_STALE_DELTA_EPS_COUNT);
    let tilt_stationary = tilt_observed_delta
        .filter(|value| value.is_finite())
        .is_some_and(|value| value.abs() <= FAILURE_DIAG_STALE_DELTA_EPS_COUNT);
    if pan_stationary && tilt_stationary {
        *streak = streak.saturating_add(1);
        return *streak >= FAILURE_DIAG_STALE_STREAK_MIN;
    }

    *streak = 0;
    false
}

fn axis_stale_detected(last_u: f64, observed_delta: Option<f64>) -> bool {
    if !last_u.is_finite() || last_u.abs() <= 0.05 {
        return false;
    }
    observed_delta
        .filter(|value| value.is_finite())
        .is_some_and(|value| value.abs() <= FAILURE_DIAG_STALE_DELTA_EPS_COUNT)
}

fn measurement_noise_hint_scale(
    model_mismatch_now: bool,
    axis_stale_now: bool,
    reversals: usize,
    measured_error: f64,
    success_tolerance: f64,
) -> f64 {
    let mut scale: f64 = 1.0;
    if model_mismatch_now {
        scale *= 1.35;
    }
    if axis_stale_now {
        scale *= 1.15;
    }
    if measured_error.is_finite()
        && success_tolerance.is_finite()
        && success_tolerance > f64::EPSILON
        && reversals >= 2
        && measured_error.abs() <= (success_tolerance * 3.0)
    {
        scale *= 1.2;
    }
    if measured_error.is_finite()
        && success_tolerance.is_finite()
        && success_tolerance > f64::EPSILON
        && measured_error.abs() <= success_tolerance
    {
        scale *= 0.96;
    }
    scale.clamp(0.8, 1.8)
}

fn dominant_axis_by_priority(
    pan_error: f64,
    tilt_error: f64,
    pan_span: f64,
    tilt_span: f64,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
    dominance_ratio: f64,
) -> Option<ControlAxis> {
    let pan_priority = axis_control_priority(pan_error, pan_span, pan_success_tolerance);
    let tilt_priority = axis_control_priority(tilt_error, tilt_span, tilt_success_tolerance);
    if pan_priority <= f64::EPSILON && tilt_priority <= f64::EPSILON {
        return None;
    }
    if pan_priority > tilt_priority * dominance_ratio {
        Some(ControlAxis::Pan)
    } else if tilt_priority > pan_priority * dominance_ratio {
        Some(ControlAxis::Tilt)
    } else {
        None
    }
}

fn axis_swap_lag_detected(
    pan_command_error: f64,
    tilt_command_error: f64,
    direction: PtzDirection,
    pan_span: f64,
    tilt_span: f64,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
) -> bool {
    let Some(dominant_axis) = dominant_axis_by_priority(
        pan_command_error,
        tilt_command_error,
        pan_span,
        tilt_span,
        pan_success_tolerance,
        tilt_success_tolerance,
        FAILURE_DIAG_AXIS_SWAP_DOMINANCE_RATIO,
    ) else {
        return false;
    };
    let Some((command_axis, _)) = control_axis_direction(direction) else {
        return false;
    };
    dominant_axis != command_axis
}

#[allow(clippy::too_many_arguments)]
fn edge_saturation_detected(
    direction: PtzDirection,
    pan_measure: f64,
    pan_min_count: f64,
    pan_max_count: f64,
    pan_span: f64,
    pan_error_measured: f64,
    pan_success_tolerance: f64,
    tilt_measure: f64,
    tilt_min_count: f64,
    tilt_max_count: f64,
    tilt_error_measured: f64,
    tilt_success_tolerance: f64,
) -> bool {
    let pan_margin = (pan_span * FAILURE_DIAG_PAN_EDGE_MARGIN_RATIO).clamp(
        FAILURE_DIAG_PAN_EDGE_MARGIN_MIN_COUNT,
        FAILURE_DIAG_PAN_EDGE_MARGIN_MAX_COUNT,
    );
    let near_pan_low = pan_measure <= (pan_min_count + pan_margin);
    let near_pan_high = pan_measure >= (pan_max_count - pan_margin);
    let pan_push_low = pan_error_measured < -pan_success_tolerance;
    let pan_push_high = pan_error_measured > pan_success_tolerance;

    let near_tilt_low = tilt_measure <= (tilt_min_count + TILT_EDGE_CONTROL_MARGIN_COUNT);
    let near_tilt_high = tilt_measure >= (tilt_max_count - TILT_EDGE_CONTROL_MARGIN_COUNT);
    let tilt_push_low = tilt_error_measured < -tilt_success_tolerance;
    let tilt_push_high = tilt_error_measured > tilt_success_tolerance;

    match direction {
        PtzDirection::Left => near_pan_low && pan_push_low,
        PtzDirection::Right => near_pan_high && pan_push_high,
        PtzDirection::Up => near_tilt_high && tilt_push_high,
        PtzDirection::Down => near_tilt_low && tilt_push_low,
        PtzDirection::LeftUp
        | PtzDirection::LeftDown
        | PtzDirection::RightUp
        | PtzDirection::RightDown => false,
    }
}

impl EdgePushContext {
    fn risky_push(self, direction: PtzDirection) -> bool {
        edge_saturation_detected(
            direction,
            self.pan_measure,
            self.pan_min_count,
            self.pan_max_count,
            self.pan_span,
            self.pan_error_measured,
            self.pan_success_tolerance,
            self.tilt_measure,
            self.tilt_min_count,
            self.tilt_max_count,
            self.tilt_error_measured,
            self.tilt_success_tolerance,
        )
    }
}

impl EdgePushLockoutState {
    fn slot_mut(&mut self, direction: PtzDirection) -> Option<&mut EdgePushDirectionLockout> {
        match direction {
            PtzDirection::Left => Some(&mut self.left),
            PtzDirection::Right => Some(&mut self.right),
            PtzDirection::Up => Some(&mut self.up),
            PtzDirection::Down => Some(&mut self.down),
            PtzDirection::LeftUp
            | PtzDirection::LeftDown
            | PtzDirection::RightUp
            | PtzDirection::RightDown => None,
        }
    }

    fn should_block_direction(&mut self, direction: PtzDirection, risky_push: bool) -> bool {
        let Some(slot) = self.slot_mut(direction) else {
            return false;
        };
        if !risky_push {
            slot.risky_streak = 0;
            slot.lockout_steps = 0;
            return false;
        }
        if slot.lockout_steps > 0 {
            slot.lockout_steps = slot.lockout_steps.saturating_sub(1);
            return true;
        }
        slot.risky_streak = slot.risky_streak.saturating_add(1);
        if slot.risky_streak < EDGE_PUSH_LOCKOUT_TRIGGER_STREAK {
            return false;
        }
        slot.risky_streak = 0;
        slot.lockout_steps = EDGE_PUSH_LOCKOUT_HOLD_STEPS;
        true
    }
}

fn fallback_command_for_edge_lockout(
    blocked_direction: PtzDirection,
    pan_command_error: f64,
    tilt_command_error: f64,
    pan_tolerance_count: f64,
    tilt_tolerance_count: f64,
) -> Option<(PtzDirection, f64)> {
    let blocked_axis = control_axis_direction(blocked_direction).map(|(axis, _)| axis)?;
    match blocked_axis {
        ControlAxis::Pan => {
            if tilt_command_error.abs() <= tilt_tolerance_count {
                None
            } else {
                command_for_axis_error(ControlAxis::Tilt, tilt_command_error)
            }
        }
        ControlAxis::Tilt => {
            if pan_command_error.abs() <= pan_tolerance_count {
                None
            } else {
                command_for_axis_error(ControlAxis::Pan, pan_command_error)
            }
        }
    }
}

fn select_command_with_edge_push_lockout(
    state: &mut EdgePushLockoutState,
    selected_command: Option<(PtzDirection, f64)>,
    pan_command_error: f64,
    tilt_command_error: f64,
    pan_tolerance_count: f64,
    tilt_tolerance_count: f64,
    edge_context: EdgePushContext,
) -> (Option<(PtzDirection, f64)>, bool) {
    let Some((direction, command_error_abs)) = selected_command else {
        return (None, false);
    };
    let risky_push = edge_context.risky_push(direction);
    if !state.should_block_direction(direction, risky_push) {
        return (Some((direction, command_error_abs)), false);
    }

    let Some((fallback_direction, fallback_error_abs)) = fallback_command_for_edge_lockout(
        direction,
        pan_command_error,
        tilt_command_error,
        pan_tolerance_count,
        tilt_tolerance_count,
    ) else {
        return (None, true);
    };
    let fallback_risky_push = edge_context.risky_push(fallback_direction);
    if state.should_block_direction(fallback_direction, fallback_risky_push) {
        (None, true)
    } else {
        (Some((fallback_direction, fallback_error_abs)), true)
    }
}

#[allow(clippy::too_many_arguments)]
fn command_from_errors(
    pan_error: f64,
    tilt_error: f64,
    pan_tolerance_count: f64,
    tilt_tolerance_count: f64,
    tie_break_pan: bool,
    pan_span: f64,
    tilt_span: f64,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
) -> Option<(PtzDirection, f64)> {
    let pan_active = pan_error.abs() > pan_tolerance_count;
    let tilt_active = tilt_error.abs() > tilt_tolerance_count;
    if !pan_active && !tilt_active {
        return None;
    }
    if pan_active && tilt_active {
        let pan_abs = pan_error.abs();
        let tilt_abs = tilt_error.abs();
        let pan_priority = axis_control_priority(pan_abs, pan_span, pan_success_tolerance);
        let tilt_priority = axis_control_priority(tilt_abs, tilt_span, tilt_success_tolerance);
        let prefer_pan = if pan_priority > tilt_priority * DUAL_AXIS_DOMINANCE_RATIO {
            true
        } else if tilt_priority > pan_priority * DUAL_AXIS_DOMINANCE_RATIO {
            false
        } else if pan_abs > TIE_BREAK_CLOSE_ERROR_COUNT || tilt_abs > TIE_BREAK_CLOSE_ERROR_COUNT {
            pan_priority >= tilt_priority
        } else {
            tie_break_pan
        };
        if prefer_pan {
            if pan_error > 0.0 {
                return Some((PtzDirection::Right, pan_abs));
            }
            return Some((PtzDirection::Left, pan_abs));
        }
        if tilt_error > 0.0 {
            return Some((PtzDirection::Up, tilt_abs));
        }
        return Some((PtzDirection::Down, tilt_abs));
    }

    if pan_active {
        if pan_error > 0.0 {
            Some((PtzDirection::Right, pan_error.abs()))
        } else {
            Some((PtzDirection::Left, pan_error.abs()))
        }
    } else if tilt_error > 0.0 {
        Some((PtzDirection::Up, tilt_error.abs()))
    } else if tilt_active {
        Some((PtzDirection::Down, tilt_error.abs()))
    } else {
        None
    }
}

fn strict_axis_focus_command(
    pan_error_measured: f64,
    tilt_error_measured: f64,
    strict_pan_tolerance: f64,
    strict_tilt_tolerance: f64,
) -> Option<(PtzDirection, f64)> {
    let pan_tolerance = if strict_pan_tolerance.is_finite() && strict_pan_tolerance > f64::EPSILON {
        strict_pan_tolerance
    } else {
        1.0
    };
    let tilt_tolerance =
        if strict_tilt_tolerance.is_finite() && strict_tilt_tolerance > f64::EPSILON {
            strict_tilt_tolerance
        } else {
            1.0
        };
    let pan_within_strict = pan_error_measured.abs() <= pan_tolerance;
    let tilt_within_strict = tilt_error_measured.abs() <= tilt_tolerance;

    if pan_within_strict && !tilt_within_strict {
        command_for_axis_error(ControlAxis::Tilt, tilt_error_measured)
    } else if tilt_within_strict && !pan_within_strict {
        command_for_axis_error(ControlAxis::Pan, pan_error_measured)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn forced_secondary_axis_command(
    state: &mut DualAxisInterleaveState,
    pan_error: f64,
    tilt_error: f64,
    pan_tolerance_count: f64,
    tilt_tolerance_count: f64,
    pan_span: f64,
    tilt_span: f64,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
    strict_pan_tolerance: f64,
    strict_tilt_tolerance: f64,
) -> Option<(PtzDirection, f64)> {
    let pan_active = pan_error.abs() > pan_tolerance_count;
    let tilt_active = tilt_error.abs() > tilt_tolerance_count;
    if !pan_active || !tilt_active {
        state.dominant_axis = None;
        state.dominant_streak = 0;
        return None;
    }

    let pan_priority = axis_control_priority(pan_error, pan_span, pan_success_tolerance);
    let tilt_priority = axis_control_priority(tilt_error, tilt_span, tilt_success_tolerance);
    if !pan_priority.is_finite() || !tilt_priority.is_finite() {
        state.dominant_axis = None;
        state.dominant_streak = 0;
        return None;
    }

    let (
        dominant_axis,
        dominant_priority,
        secondary_axis,
        secondary_priority,
        secondary_error,
        secondary_success_tolerance,
        secondary_strict_tolerance,
    ) = if pan_priority >= tilt_priority {
        (
            ControlAxis::Pan,
            pan_priority,
            ControlAxis::Tilt,
            tilt_priority,
            tilt_error,
            tilt_success_tolerance,
            strict_tilt_tolerance,
        )
    } else {
        (
            ControlAxis::Tilt,
            tilt_priority,
            ControlAxis::Pan,
            pan_priority,
            pan_error,
            pan_success_tolerance,
            strict_pan_tolerance,
        )
    };

    let fallback_tolerance = match secondary_axis {
        ControlAxis::Pan => pan_tolerance_count,
        ControlAxis::Tilt => tilt_tolerance_count,
    };
    let success_tolerance =
        if secondary_success_tolerance.is_finite() && secondary_success_tolerance > f64::EPSILON {
            secondary_success_tolerance
        } else {
            fallback_tolerance
        };
    let strict_tolerance =
        if secondary_strict_tolerance.is_finite() && secondary_strict_tolerance > f64::EPSILON {
            secondary_strict_tolerance
        } else {
            fallback_tolerance
        };
    let strict_endgame =
        pan_error.abs().max(tilt_error.abs()) <= SECONDARY_AXIS_STRICT_ENDGAME_ERROR_COUNT;
    let secondary_tolerance = if strict_endgame {
        success_tolerance.min(strict_tolerance)
    } else {
        success_tolerance
    };
    if secondary_error.abs() <= secondary_tolerance {
        state.dominant_axis = Some(dominant_axis);
        state.dominant_streak = 0;
        return None;
    }

    if state.dominant_axis != Some(dominant_axis) {
        state.dominant_axis = Some(dominant_axis);
        state.dominant_streak = 0;
    }
    let interval = secondary_axis_interleave_interval(dominant_priority, secondary_priority);
    if state.dominant_streak < interval {
        state.dominant_streak = state.dominant_streak.saturating_add(1);
        return None;
    }

    state.dominant_streak = 0;
    command_for_axis_error(secondary_axis, secondary_error)
}

fn secondary_axis_interleave_interval(dominant_priority: f64, secondary_priority: f64) -> usize {
    if !dominant_priority.is_finite()
        || !secondary_priority.is_finite()
        || secondary_priority <= f64::EPSILON
    {
        return SECONDARY_AXIS_INTERLEAVE_INTERVAL_MAX;
    }
    let ratio = (dominant_priority / secondary_priority).clamp(1.0, 32.0);
    let interval = if ratio <= 1.25 {
        1
    } else if ratio <= 1.8 {
        2
    } else if ratio <= 2.6 {
        3
    } else if ratio <= 4.0 {
        4
    } else {
        5
    };
    interval.clamp(
        SECONDARY_AXIS_INTERLEAVE_INTERVAL_MIN,
        SECONDARY_AXIS_INTERLEAVE_INTERVAL_MAX,
    )
}

fn command_for_axis_error(axis: ControlAxis, axis_error: f64) -> Option<(PtzDirection, f64)> {
    if !axis_error.is_finite() {
        return None;
    }
    let abs_error = axis_error.abs();
    if abs_error <= f64::EPSILON {
        return None;
    }
    match axis {
        ControlAxis::Pan => {
            if axis_error.is_sign_positive() {
                Some((PtzDirection::Right, abs_error))
            } else {
                Some((PtzDirection::Left, abs_error))
            }
        }
        ControlAxis::Tilt => {
            if axis_error.is_sign_positive() {
                Some((PtzDirection::Up, abs_error))
            } else {
                Some((PtzDirection::Down, abs_error))
            }
        }
    }
}

fn speed_cap_for_error(error_abs: f64) -> u8 {
    if error_abs <= FINE_CONTROL_ERROR_COUNT {
        1
    } else if error_abs <= COARSE_CONTROL_ERROR_COUNT {
        2
    } else if error_abs <= 900.0 {
        3
    } else if error_abs <= 1_500.0 {
        4
    } else if error_abs <= 2_500.0 {
        6
    } else {
        8
    }
}

fn control_pulse_ms_for_error(error_abs: f64) -> u64 {
    let pulse_ms = if error_abs <= MICRO_CONTROL_ERROR_COUNT {
        0
    } else if error_abs <= FINE_CONTROL_ERROR_COUNT {
        20
    } else if error_abs <= COARSE_CONTROL_ERROR_COUNT {
        35
    } else if error_abs <= 520.0 {
        55
    } else if error_abs <= 900.0 {
        90
    } else if error_abs <= 1_500.0 {
        110
    } else {
        140
    };
    pulse_ms.clamp(MIN_CONTROL_PULSE_MS, MAX_CONTROL_PULSE_MS)
}

fn pulse_ms_for_direction_with_lut(
    direction: PtzDirection,
    pan_command_error: f64,
    tilt_command_error: f64,
    pan_lut: &AxisPulseLut,
    tilt_lut: &AxisPulseLut,
    command_error_abs: f64,
    edge_band: bool,
) -> u64 {
    let fallback = control_pulse_ms_for_error(command_error_abs);
    let Some((axis, axis_direction)) = control_axis_direction(direction) else {
        return fallback;
    };
    let axis_error = match axis {
        ControlAxis::Pan => pan_command_error,
        ControlAxis::Tilt => tilt_command_error,
    };
    if !axis_error.is_finite() {
        return fallback;
    }

    let target_count = (axis_error.abs() * PULSE_LUT_TARGET_GAIN)
        .clamp(PULSE_LUT_TARGET_MIN_COUNT, PULSE_LUT_TARGET_MAX_COUNT);
    let lut = match axis {
        ControlAxis::Pan => pan_lut,
        ControlAxis::Tilt => tilt_lut,
    };
    lut.pulse_ms_for_target_in_band(
        axis_direction,
        edge_band,
        target_count,
        PULSE_LUT_MIN_MS,
        PULSE_LUT_MAX_MS,
    )
}

fn control_axis_direction(direction: PtzDirection) -> Option<(ControlAxis, AxisDirection)> {
    match direction {
        PtzDirection::Left => Some((ControlAxis::Pan, AxisDirection::Negative)),
        PtzDirection::Right => Some((ControlAxis::Pan, AxisDirection::Positive)),
        PtzDirection::Up => Some((ControlAxis::Tilt, AxisDirection::Positive)),
        PtzDirection::Down => Some((ControlAxis::Tilt, AxisDirection::Negative)),
        PtzDirection::LeftUp
        | PtzDirection::LeftDown
        | PtzDirection::RightUp
        | PtzDirection::RightDown => None,
    }
}

fn pulse_lut_edge_band_for_command(
    direction: PtzDirection,
    pan_measure: f64,
    pan_min_count: f64,
    pan_max_count: f64,
    tilt_measure: f64,
    tilt_min_count: f64,
    tilt_max_count: f64,
) -> bool {
    let band = match control_axis_direction(direction).map(|(axis, _)| axis) {
        Some(ControlAxis::Pan) => classify_position_band(pan_measure, pan_min_count, pan_max_count),
        Some(ControlAxis::Tilt) => {
            classify_position_band(tilt_measure, tilt_min_count, tilt_max_count)
        }
        None => PositionBand::Mid,
    };
    matches!(band, PositionBand::Low | PositionBand::High)
}

fn pulse_lut_learning_allowed(
    direction: PtzDirection,
    pulse_lut_candidate: bool,
    pulse_ms: u64,
    pan_reversal_now: bool,
    tilt_reversal_now: bool,
) -> bool {
    if !pulse_lut_candidate || pulse_ms < PULSE_LUT_MIN_MS {
        return false;
    }
    match control_axis_direction(direction).map(|(axis, _)| axis) {
        Some(ControlAxis::Pan) => !pan_reversal_now,
        Some(ControlAxis::Tilt) => !tilt_reversal_now,
        None => false,
    }
}

fn pending_pulse_observation_for_command(
    direction: PtzDirection,
    pulse_ms: u64,
) -> Option<PendingPulseObservation> {
    if pulse_ms == 0 {
        return None;
    }
    let (axis, axis_direction) = control_axis_direction(direction)?;
    Some(PendingPulseObservation {
        axis,
        direction: axis_direction,
        pulse_ms,
        edge_band: false,
    })
}

fn pending_pulse_observation_for_lut_command(
    direction: PtzDirection,
    pulse_ms: u64,
    edge_band: bool,
    learning_enabled: bool,
) -> Option<PendingPulseObservation> {
    if !learning_enabled {
        return None;
    }
    let mut observation = pending_pulse_observation_for_command(direction, pulse_ms)?;
    observation.edge_band = edge_band;
    Some(observation)
}

fn apply_pending_pulse_observation(
    pending: &mut Option<PendingPulseObservation>,
    pan_observed_delta: Option<f64>,
    tilt_observed_delta: Option<f64>,
    pan_sample_reliable: bool,
    tilt_sample_reliable: bool,
    pan_lut: &mut AxisPulseLut,
    tilt_lut: &mut AxisPulseLut,
) {
    let Some(observation) = pending.take() else {
        return;
    };
    let observed_delta = match observation.axis {
        ControlAxis::Pan => pan_observed_delta,
        ControlAxis::Tilt => tilt_observed_delta,
    };
    let Some(observed_delta) = observed_delta else {
        return;
    };
    if !observation.direction.matches_observed_delta(observed_delta) {
        return;
    }

    match observation.axis {
        ControlAxis::Pan if pan_sample_reliable => pan_lut.update_in_band(
            observation.direction,
            observation.edge_band,
            observation.pulse_ms,
            observed_delta,
        ),
        ControlAxis::Tilt if tilt_sample_reliable => tilt_lut.update_in_band(
            observation.direction,
            observation.edge_band,
            observation.pulse_ms,
            observed_delta,
        ),
        ControlAxis::Pan | ControlAxis::Tilt => {}
    }
}

fn position_stable_threshold_count(
    tolerance_count: f64,
    pan_deadband_hint_count: f64,
    tilt_deadband_hint_count: f64,
) -> f64 {
    let tolerance_based = (tolerance_count * BACKEND_POSITION_STABLE_TOLERANCE_RATIO).clamp(
        BACKEND_POSITION_STABLE_MIN_COUNT,
        BACKEND_POSITION_STABLE_MAX_COUNT,
    );
    let deadband_based = (pan_deadband_hint_count.max(tilt_deadband_hint_count) * 0.12).clamp(
        BACKEND_POSITION_STABLE_MIN_COUNT,
        BACKEND_POSITION_STABLE_MAX_COUNT,
    );
    tolerance_based.max(deadband_based)
}

fn near_target_speed1_pulse_ms(
    base_pulse_ms: u64,
    command_error_abs: f64,
    direction: PtzDirection,
) -> u64 {
    let is_tilt_axis = matches!(direction, PtzDirection::Up | PtzDirection::Down);
    if is_tilt_axis && command_error_abs <= NEAR_TARGET_TILT_MICRO_ERROR_COUNT {
        return 0;
    }
    base_pulse_ms.clamp(0, NEAR_TARGET_SPEED1_MAX_PULSE_MS)
}

fn ensure_active_command_pulse_ms(
    pulse_ms: u64,
    direction: PtzDirection,
    pan_command_error: f64,
    tilt_command_error: f64,
    strict_pan_tolerance: f64,
    strict_tilt_tolerance: f64,
) -> u64 {
    if pulse_ms > 0 {
        return pulse_ms;
    }
    let Some((axis, _)) = control_axis_direction(direction) else {
        return pulse_ms;
    };
    let (axis_error, strict_tolerance) = match axis {
        ControlAxis::Pan => (pan_command_error.abs(), strict_pan_tolerance),
        ControlAxis::Tilt => (tilt_command_error.abs(), strict_tilt_tolerance),
    };
    let strict_tolerance = if strict_tolerance.is_finite() && strict_tolerance > f64::EPSILON {
        strict_tolerance
    } else {
        1.0
    };
    if axis_error > strict_tolerance {
        ACTIVE_COMMAND_MIN_PULSE_MS
    } else {
        pulse_ms
    }
}

fn clamp_pan_reversal_micro_control(
    direction: PtzDirection,
    speed: u8,
    pulse_ms: u64,
    pan_reversal_now: bool,
    pan_error_measured: f64,
) -> (u8, u64) {
    if !pan_reversal_now {
        return (speed, pulse_ms);
    }
    if !matches!(direction, PtzDirection::Left | PtzDirection::Right) {
        return (speed, pulse_ms);
    }
    if !pan_error_measured.is_finite() || pan_error_measured.abs() > PAN_REVERSAL_MICRO_ERROR_COUNT
    {
        return (speed, pulse_ms);
    }
    (speed.min(1), pulse_ms.min(PAN_REVERSAL_MICRO_MAX_PULSE_MS))
}

fn clamp_tilt_reversal_micro_control(
    direction: PtzDirection,
    speed: u8,
    pulse_ms: u64,
    tilt_reversal_now: bool,
    tilt_error_measured: f64,
    tilt_deadband_hint: f64,
) -> (u8, u64) {
    if !tilt_reversal_now {
        return (speed, pulse_ms);
    }
    if !matches!(direction, PtzDirection::Up | PtzDirection::Down) {
        return (speed, pulse_ms);
    }
    let near_band = if tilt_deadband_hint.is_finite() && tilt_deadband_hint > 0.0 {
        (tilt_deadband_hint * 3.0).clamp(24.0, 220.0)
    } else {
        48.0
    };
    if !tilt_error_measured.is_finite() || tilt_error_measured.abs() > near_band {
        return (speed, pulse_ms);
    }
    (speed.min(1), pulse_ms.min(10))
}

fn apply_tilt_backlash_compensation(
    command_error: f64,
    measured_error: f64,
    deadband_hint: f64,
    tilt_reversal_now: bool,
) -> f64 {
    if !tilt_reversal_now || !command_error.is_finite() || !measured_error.is_finite() {
        return command_error;
    }
    if measured_error.abs() <= f64::EPSILON {
        return command_error;
    }
    let deadband_floor = if deadband_hint.is_finite() && deadband_hint > 0.0 {
        deadband_hint.min(140.0)
    } else {
        0.0
    };
    if deadband_floor <= f64::EPSILON || command_error.abs() >= deadband_floor {
        return command_error;
    }
    if measured_error.is_sign_positive() {
        deadband_floor
    } else {
        -deadband_floor
    }
}

fn clamp_tilt_edge_control(
    direction: PtzDirection,
    speed: u8,
    pulse_ms: u64,
    tilt_measure: f64,
    tilt_min_count: f64,
    tilt_max_count: f64,
) -> (u8, u64) {
    let near_upper_edge = tilt_measure >= (tilt_max_count - TILT_EDGE_CONTROL_MARGIN_COUNT);
    let near_lower_edge = tilt_measure <= (tilt_min_count + TILT_EDGE_CONTROL_MARGIN_COUNT);
    let edge_risk = matches!(direction, PtzDirection::Up) && near_upper_edge
        || matches!(direction, PtzDirection::Down) && near_lower_edge;
    if !edge_risk {
        return (speed, pulse_ms);
    }

    (
        speed.min(TILT_EDGE_CONTROL_SPEED_CAP),
        pulse_ms.min(TILT_EDGE_CONTROL_MAX_PULSE_MS),
    )
}

impl AxisDirection {
    fn matches_observed_delta(self, observed_delta: f64) -> bool {
        if !observed_delta.is_finite() {
            return false;
        }
        match self {
            AxisDirection::Positive => observed_delta > 0.0,
            AxisDirection::Negative => observed_delta < 0.0,
        }
    }
}

fn control_step_ms_for_error(error_abs: f64) -> u64 {
    let base = if error_abs <= MICRO_CONTROL_ERROR_COUNT {
        140
    } else if error_abs <= FINE_CONTROL_ERROR_COUNT {
        130
    } else if error_abs <= COARSE_CONTROL_ERROR_COUNT {
        120
    } else if error_abs <= 900.0 {
        110
    } else {
        130
    };
    base.max(SETTLE_STEP_MS)
}

fn remaining_control_step_sleep_duration(step_budget_ms: u64, elapsed: Duration) -> Duration {
    Duration::from_millis(step_budget_ms).saturating_sub(elapsed)
}

fn format_transport_hint(hint: Option<ptz_transport::TransportMotionHint>) -> String {
    let Some(hint) = hint else {
        return "none".to_string();
    };

    let moving = match hint.moving {
        Some(true) => "moving",
        Some(false) => "stopped",
        None => "unknown",
    };
    let age = hint
        .move_age_ms
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "n/a".to_string());
    format!("{moving}@{age}")
}

fn apply_fine_phase_feedforward(
    command_error: f64,
    model_beta: f64,
    last_u: f64,
    observed_delta: Option<f64>,
    tolerance_count: f64,
) -> f64 {
    if !command_error.is_finite()
        || !model_beta.is_finite()
        || !last_u.is_finite()
        || last_u.abs() <= f64::EPSILON
    {
        return command_error;
    }
    let Some(observed_delta) = observed_delta.filter(|value| value.is_finite()) else {
        return command_error;
    };

    let predicted_delta = model_beta * last_u;
    let residual_bias = predicted_delta - observed_delta;
    let feedforward_limit = (tolerance_count * 2.2).clamp(14.0, FINE_FEEDFORWARD_MAX_COUNT);
    let feedforward =
        (residual_bias * FINE_FEEDFORWARD_GAIN).clamp(-feedforward_limit, feedforward_limit);
    command_error + feedforward
}

fn relative_delta_from_error(command_error: f64, tolerance_count: f64) -> i64 {
    if !command_error.is_finite() || command_error.abs() <= tolerance_count {
        return 0;
    }

    let magnitude = (command_error.abs() * FINE_RELATIVE_STEP_GAIN)
        .clamp(FINE_RELATIVE_STEP_MIN_COUNT, FINE_RELATIVE_STEP_MAX_COUNT)
        .round() as i64;
    if command_error.is_sign_positive() {
        magnitude
    } else {
        -magnitude
    }
}

fn control_components_from_relative_step(
    pan_delta_count: i64,
    tilt_delta_count: i64,
) -> (f64, f64) {
    let axis_component = |delta_count: i64| -> f64 {
        if delta_count == 0 {
            return 0.0;
        }
        let magnitude =
            ((delta_count.unsigned_abs() as f64) / FINE_RELATIVE_STEP_MAX_COUNT).clamp(0.2, 0.9);
        if delta_count.is_positive() {
            magnitude
        } else {
            -magnitude
        }
    };

    (
        axis_component(pan_delta_count),
        axis_component(tilt_delta_count),
    )
}

fn apply_reversal_guard(
    error: f64,
    last_u: f64,
    tolerance_count: f64,
    deadband_hint_count: f64,
) -> f64 {
    if !error.is_finite() {
        return error;
    }
    if error.abs() <= tolerance_count {
        return error;
    }
    if !last_u.is_finite() || last_u.abs() <= f64::EPSILON {
        return error;
    }

    let reversing = error.signum() != last_u.signum();
    if !reversing {
        return error;
    }

    let momentum_scale = last_u.abs().clamp(REVERSAL_GUARD_MOMENTUM_MIN_SCALE, 1.0);
    let deadband_ratio = dynamic_guard_deadband_ratio(deadband_hint_count, tolerance_count);
    let guard_threshold = (tolerance_count * REVERSAL_GUARD_MULTIPLIER)
        .max(REVERSAL_GUARD_MIN_COUNT)
        .max(deadband_hint_count * deadband_ratio)
        * momentum_scale;
    let near_target_threshold = (tolerance_count * REVERSAL_GUARD_NEAR_TARGET_RATIO)
        .max(REVERSAL_GUARD_NEAR_TARGET_MIN_COUNT)
        .min(guard_threshold);
    if error.abs() <= near_target_threshold {
        let blend = ((error.abs() - tolerance_count)
            / (near_target_threshold - tolerance_count).max(1.0))
        .clamp(0.0, 1.0);
        let scale = REVERSAL_GUARD_NEAR_TARGET_MIN_SCALE
            + ((1.0 - REVERSAL_GUARD_NEAR_TARGET_MIN_SCALE) * blend);
        return error * scale;
    }
    if error.abs() <= guard_threshold {
        let floor = (tolerance_count * REVERSAL_GUARD_MICRO_FLOOR_MIN_SCALE)
            .max(deadband_hint_count * 0.25)
            .clamp(1.0, REVERSAL_GUARD_MICRO_FLOOR_MAX_COUNT)
            .min(error.abs());
        if error.is_sign_positive() {
            floor
        } else {
            -floor
        }
    } else {
        error
    }
}

fn enforce_residual_command_activity(
    command_error: f64,
    measured_error: f64,
    control_tolerance: f64,
    success_tolerance: f64,
) -> f64 {
    if !command_error.is_finite() || !measured_error.is_finite() {
        return command_error;
    }
    if measured_error.abs() <= success_tolerance {
        return command_error;
    }
    if command_error.abs() > control_tolerance {
        return command_error;
    }
    if command_error.abs() > f64::EPSILON && command_error.signum() != measured_error.signum() {
        return command_error;
    }

    let activation_floor = control_tolerance.max(success_tolerance) + 1.0;
    if measured_error.is_sign_positive() {
        measured_error.abs().max(activation_floor)
    } else {
        -measured_error.abs().max(activation_floor)
    }
}

fn dynamic_guard_deadband_ratio(deadband_hint_count: f64, tolerance_count: f64) -> f64 {
    if deadband_hint_count <= 0.0 {
        return CALIBRATION_GUARD_DEADBAND_RATIO;
    }
    let hint_ratio = (deadband_hint_count / tolerance_count.max(1.0)).clamp(0.5, 3.0);
    (CALIBRATION_GUARD_DEADBAND_RATIO + ((hint_ratio - 1.0) * 0.1)).clamp(0.25, 0.75)
}

fn reversal_detected(
    previous_error: Option<f64>,
    current_error: f64,
    tolerance_count: f64,
    success_tolerance_count: f64,
) -> bool {
    let previous = previous_error.unwrap_or(current_error);
    let success_band = if success_tolerance_count.is_finite() && success_tolerance_count > 0.0 {
        success_tolerance_count
    } else {
        tolerance_count
    };
    let detect_range =
        (success_band * OSCILLATION_DETECT_RANGE_MULTIPLIER).max(OSCILLATION_MIN_DETECT_COUNT);

    let previous_in_band = previous.abs() > tolerance_count && previous.abs() <= detect_range;
    let current_in_band =
        current_error.abs() > tolerance_count && current_error.abs() <= detect_range;
    let sign_flipped = previous.signum() != current_error.signum()
        && previous.abs() > f64::EPSILON
        && current_error.abs() > f64::EPSILON;

    sign_flipped && previous_in_band && current_in_band
}

fn update_reversal_counter(
    counter: &mut usize,
    previous_error: &mut Option<f64>,
    current_error: f64,
    tolerance_count: f64,
    success_tolerance_count: f64,
) {
    let success_band = if success_tolerance_count.is_finite() && success_tolerance_count > 0.0 {
        success_tolerance_count
    } else {
        tolerance_count
    };
    let detect_range =
        (success_band * OSCILLATION_DETECT_RANGE_MULTIPLIER).max(OSCILLATION_MIN_DETECT_COUNT);

    if reversal_detected(
        *previous_error,
        current_error,
        tolerance_count,
        success_tolerance_count,
    ) {
        *counter = counter.saturating_add(1);
    } else if current_error.abs() > detect_range {
        *counter = 0;
    } else {
        *counter = counter.saturating_sub(1);
    }
    *previous_error = Some(current_error);
}

fn oscillation_damping_active(
    pan_reversals: usize,
    tilt_reversals: usize,
    pan_error_measured: f64,
    tilt_error_measured: f64,
    pan_success_tolerance: f64,
    tilt_success_tolerance: f64,
) -> bool {
    let reversal_sum = pan_reversals.saturating_add(tilt_reversals);
    if reversal_sum < OSCILLATION_DAMPING_REVERSAL_SUM {
        return false;
    }
    if !pan_error_measured.is_finite() || !tilt_error_measured.is_finite() {
        return false;
    }
    if !pan_success_tolerance.is_finite()
        || !tilt_success_tolerance.is_finite()
        || pan_success_tolerance <= 0.0
        || tilt_success_tolerance <= 0.0
    {
        return false;
    }

    pan_error_measured.abs()
        <= (pan_success_tolerance * OSCILLATION_DAMPING_SUCCESS_BAND_MULTIPLIER)
        && tilt_error_measured.abs()
            <= (tilt_success_tolerance * OSCILLATION_DAMPING_SUCCESS_BAND_MULTIPLIER)
}

fn adaptive_axis_tolerance(
    base_tolerance: f64,
    reversal_count: usize,
    deadband_hint_count: f64,
) -> f64 {
    if reversal_count >= OSCILLATION_REVERSAL_THRESHOLD {
        let relaxation = (deadband_hint_count * OSCILLATION_TOLERANCE_RELAX_RATIO)
            .min(OSCILLATION_TOLERANCE_RELAX_MAX_COUNT);
        base_tolerance + relaxation
    } else {
        base_tolerance
    }
}

fn required_stable_steps_for_oscillation(pan_reversals: usize, tilt_reversals: usize) -> usize {
    let reversal_sum = pan_reversals.saturating_add(tilt_reversals);
    if reversal_sum >= OSCILLATION_STABLE_STEP_REDUCTION_REVERSAL_SUM {
        1
    } else {
        REQUIRED_STABLE_STEPS
    }
}

fn control_components_from_command(
    direction: PtzDirection,
    speed: u8,
    pulse_ms: u64,
) -> (f64, f64) {
    let speed_factor = (speed as f64 / 64.0).clamp(0.0, 1.0);
    let pulse_factor = (pulse_ms as f64 / 120.0).clamp(0.5, 1.5);
    let normalized = (speed_factor * pulse_factor).clamp(0.0, 1.0);
    match direction {
        PtzDirection::Left => (-normalized, 0.0),
        PtzDirection::Right => (normalized, 0.0),
        PtzDirection::Up => (0.0, normalized),
        PtzDirection::Down => (0.0, -normalized),
        PtzDirection::LeftUp => (-normalized, normalized),
        PtzDirection::LeftDown => (-normalized, -normalized),
        PtzDirection::RightUp => (normalized, normalized),
        PtzDirection::RightDown => (normalized, -normalized),
    }
}

fn consider_best(
    best: &mut Option<BestObservedState>,
    pan_count: i64,
    tilt_count: i64,
    target_pan_count: i64,
    target_tilt_count: i64,
) -> bool {
    let candidate_pan_abs_error = (target_pan_count - pan_count).abs();
    let candidate_tilt_abs_error = (target_tilt_count - tilt_count).abs();
    let candidate_max_error = candidate_pan_abs_error.max(candidate_tilt_abs_error);
    let candidate_sum_error = candidate_pan_abs_error + candidate_tilt_abs_error;

    let should_update = match best {
        Some(current) => {
            let current_max_error = current.pan_abs_error.max(current.tilt_abs_error);
            let current_sum_error = current.pan_abs_error + current.tilt_abs_error;
            candidate_max_error < current_max_error
                || (candidate_max_error == current_max_error
                    && candidate_sum_error < current_sum_error)
        }
        None => true,
    };

    if should_update {
        *best = Some(BestObservedState {
            pan_count,
            tilt_count,
            pan_abs_error: candidate_pan_abs_error,
            tilt_abs_error: candidate_tilt_abs_error,
        });
        true
    } else {
        false
    }
}

fn ekf_state_identity(client: &Client, channel: u8) -> (String, PathBuf) {
    let endpoint_key = sanitize_key_component(client.endpoint());
    let state_key = format!("{endpoint_key}.ch{channel}");
    let file_name = format!("{state_key}.ekf-count.json");
    (
        state_key,
        runtime::calibration_dir_from_env().join(file_name),
    )
}

fn load_stored_ekf_state(
    path: &Path,
    state_key: &str,
    channel: u8,
) -> AppResult<Option<StoredPtzEkfState>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                format!("failed to read EKF state file {}: {error}", path.display()),
            ));
        }
    };

    let parsed = match serde_json::from_str::<StoredPtzEkfState>(&raw) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    if parsed.schema_version != EKF_STATE_SCHEMA_VERSION {
        return Ok(None);
    }
    if parsed.channel != channel {
        return Ok(None);
    }
    if parsed.state_key != state_key {
        return Ok(None);
    }
    if !parsed.is_finite() {
        return Ok(None);
    }

    Ok(Some(parsed))
}

fn save_stored_ekf_state(
    path: &Path,
    state_key: &str,
    channel: u8,
    pan_filter: &AxisEkf,
    tilt_filter: &AxisEkf,
    learning: OnlineLearningState<'_>,
) -> AppResult<()> {
    let stored = StoredPtzEkfState {
        schema_version: EKF_STATE_SCHEMA_VERSION,
        state_key: state_key.to_string(),
        channel,
        updated_at: now_epoch_millis(),
        last_pan_u: learning.last_pan_u.clamp(-1.0, 1.0),
        last_tilt_u: learning.last_tilt_u.clamp(-1.0, 1.0),
        pan: StoredAxisEkfState::from_snapshot(pan_filter.snapshot()),
        tilt: StoredAxisEkfState::from_snapshot(tilt_filter.snapshot()),
        pan_positive_beta: Some(learning.pan_gain_tracker.positive_beta()),
        pan_negative_beta: Some(learning.pan_gain_tracker.negative_beta()),
        tilt_positive_beta: Some(learning.tilt_gain_tracker.positive_beta()),
        tilt_negative_beta: Some(learning.tilt_gain_tracker.negative_beta()),
        pan_positive_counts_per_ms: Some(
            learning
                .pan_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Positive, false),
        ),
        pan_negative_counts_per_ms: Some(
            learning
                .pan_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Negative, false),
        ),
        pan_positive_edge_counts_per_ms: Some(
            learning
                .pan_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Positive, true),
        ),
        pan_negative_edge_counts_per_ms: Some(
            learning
                .pan_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Negative, true),
        ),
        tilt_positive_counts_per_ms: Some(
            learning
                .tilt_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Positive, false),
        ),
        tilt_negative_counts_per_ms: Some(
            learning
                .tilt_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Negative, false),
        ),
        tilt_positive_edge_counts_per_ms: Some(
            learning
                .tilt_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Positive, true),
        ),
        tilt_negative_edge_counts_per_ms: Some(
            learning
                .tilt_pulse_lut
                .counts_per_ms_in_band(AxisDirection::Negative, true),
        ),
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "failed to create EKF state directory {}: {error}",
                    parent.display()
                ),
            )
        })?;
    }

    let serialized = serde_json::to_string_pretty(&stored).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to serialize EKF state JSON: {error}"),
        )
    })?;
    fs::write(path, serialized).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to write EKF state file {}: {error}", path.display()),
        )
    })
}

fn finalize_with_best_effort_stop<T>(
    client: &Client,
    channel: u8,
    force_cgi_pulse_transport: bool,
    result: AppResult<T>,
) -> AppResult<T> {
    let stop_error = stop_ptz_for_absolute(client, channel, force_cgi_pulse_transport).err();

    match result {
        Ok(value) => {
            if let Some(error) = stop_error {
                return Err(AppError::new(
                    ErrorKind::UnexpectedResponse,
                    format!(
                        "set_absolute_raw completed but failed to send Stop on channel {channel}: {}",
                        error.message
                    ),
                ));
            }
            Ok(value)
        }
        Err(mut error) => {
            if let Some(stop_error) = stop_error {
                error.message = format!(
                    "{} (also failed to send Stop on channel {channel}: {})",
                    error.message, stop_error.message
                );
            }
            Err(error)
        }
    }
}

fn move_ptz_for_absolute(
    client: &Client,
    channel: u8,
    direction: PtzDirection,
    speed: u8,
    pulse_ms: u64,
    force_cgi_pulse_transport: bool,
) -> AppResult<()> {
    if force_cgi_pulse_transport {
        ptz::move_ptz(client, channel, direction, speed, Some(pulse_ms))
    } else {
        ptz_transport::move_ptz(client, channel, direction, speed, Some(pulse_ms))
    }
}

fn stop_ptz_for_absolute(
    client: &Client,
    channel: u8,
    force_cgi_pulse_transport: bool,
) -> AppResult<()> {
    if force_cgi_pulse_transport {
        ptz::stop_ptz(client, channel)
    } else {
        ptz_transport::stop_ptz(client, channel)
    }
}

fn should_force_cgi_pulse_transport(client: &Client, channel: u8) -> bool {
    if runtime::ptz_backend_from_env() != PtzBackend::OnvifContinuous {
        return false;
    }
    let onvif_options = ptz_transport::get_onvif_configuration_options(client, channel)
        .ok()
        .flatten();
    should_force_cgi_for_onvif_options(onvif_options.as_ref())
}

fn should_force_cgi_for_onvif_options(options: Option<&OnvifPtzConfigurationOptions>) -> bool {
    let Some(options) = options else {
        return false;
    };
    if options.supports_relative_pan_tilt_translation || !options.has_timeout_range {
        return false;
    }
    options
        .timeout_min
        .as_deref()
        .and_then(parse_onvif_duration_ms)
        .is_some_and(|min_ms| min_ms >= 1_000)
}

fn parse_onvif_duration_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let seconds = raw
        .strip_prefix("PT")
        .and_then(|value| value.strip_suffix('S'))
        .and_then(|value| value.parse::<f64>().ok())?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds * 1_000.0).round() as u64)
}

fn validate_inputs(tolerance_count: i64, timeout_ms: u64) -> AppResult<()> {
    if tolerance_count <= 0 {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "tolerance count must be greater than 0",
        ));
    }
    if timeout_ms == 0 {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "timeout_ms must be greater than 0",
        ));
    }
    Ok(())
}

fn sanitize_key_component(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut previous_was_separator = false;

    for character in raw.trim().chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            normalized.push('_');
            previous_was_separator = true;
        }
    }

    let trimmed = normalized.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl StoredAxisEkfState {
    fn from_snapshot(snapshot: AxisEkfSnapshot) -> Self {
        Self {
            position: snapshot.state.position,
            velocity: snapshot.state.velocity,
            bias: snapshot.state.bias,
            covariance: snapshot.covariance,
            adaptive_r: Some(snapshot.adaptive_r),
            adaptive_q_scale: Some(snapshot.adaptive_q_scale),
            last_nis: snapshot.last_nis,
            ewma_nis: snapshot.ewma_nis,
            residual_variance_proxy: snapshot.residual_variance_proxy,
        }
    }

    fn to_snapshot(&self) -> AxisEkfSnapshot {
        AxisEkfSnapshot {
            state: crate::core::model::AxisState {
                position: self.position,
                velocity: self.velocity,
                bias: self.bias,
            },
            covariance: self.covariance,
            adaptive_r: self.adaptive_r.unwrap_or(1.0),
            adaptive_q_scale: self.adaptive_q_scale.unwrap_or(1.0),
            last_nis: self.last_nis,
            ewma_nis: self.ewma_nis,
            residual_variance_proxy: self.residual_variance_proxy,
        }
    }

    fn is_finite(&self) -> bool {
        let adaptive_r_ok = self.adaptive_r.is_none_or(|value| value.is_finite());
        let adaptive_q_ok = self.adaptive_q_scale.is_none_or(|value| value.is_finite());
        let last_nis_ok = self.last_nis.is_none_or(|value| value.is_finite());
        let ewma_nis_ok = self.ewma_nis.is_none_or(|value| value.is_finite());
        let residual_variance_ok = self
            .residual_variance_proxy
            .is_none_or(|value| value.is_finite());
        self.position.is_finite()
            && self.velocity.is_finite()
            && self.bias.is_finite()
            && adaptive_r_ok
            && adaptive_q_ok
            && last_nis_ok
            && ewma_nis_ok
            && residual_variance_ok
            && self
                .covariance
                .iter()
                .all(|row| row.iter().all(|value| value.is_finite()))
    }
}

impl StoredPtzEkfState {
    fn is_finite(&self) -> bool {
        let pan_positive_ok = self
            .pan_positive_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let pan_negative_ok = self
            .pan_negative_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let pan_positive_edge_ok = self
            .pan_positive_edge_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let pan_negative_edge_ok = self
            .pan_negative_edge_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let tilt_positive_ok = self
            .tilt_positive_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let tilt_negative_ok = self
            .tilt_negative_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let tilt_positive_edge_ok = self
            .tilt_positive_edge_counts_per_ms
            .is_none_or(|value| value.is_finite());
        let tilt_negative_edge_ok = self
            .tilt_negative_edge_counts_per_ms
            .is_none_or(|value| value.is_finite());
        self.last_pan_u.is_finite()
            && self.last_tilt_u.is_finite()
            && self.pan.is_finite()
            && self.tilt.is_finite()
            && pan_positive_ok
            && pan_negative_ok
            && pan_positive_edge_ok
            && pan_negative_edge_ok
            && tilt_positive_ok
            && tilt_negative_ok
            && tilt_positive_edge_ok
            && tilt_negative_edge_ok
    }
}

#[cfg(test)]
mod tests;
