use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app::usecases::ptz_calibrate_auto;
use crate::app::usecases::ptz_controller::{AxisEkf, AxisEkfConfig, AxisEkfSnapshot};
use crate::app::usecases::ptz_get_absolute::PtzAbsolutePosition;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{AxisModelParams, NumericRange, PtzDirection};
use crate::reolink::client::Client;
use crate::reolink::ptz;
use serde::{Deserialize, Serialize};

const SETTLE_STEP_MS: u64 = 80;
const EKF_TS_SEC: f64 = 0.05;
const EKF_STATE_SCHEMA_VERSION: u32 = 1;
const DEFAULT_PAN_MIN_DEG: f64 = -220.0;
const DEFAULT_PAN_MAX_DEG: f64 = 220.0;
const DEFAULT_TILT_MIN_DEG: f64 = -120.0;
const DEFAULT_TILT_MAX_DEG: f64 = 120.0;
const EKF_POSITION_MARGIN_DEG: f64 = 5.0;
const EKF_MAX_BIAS_RATIO: f64 = 0.08;
const EKF_MIN_BIAS_DEG: f64 = 2.0;
const EKF_MAX_BIAS_DEG: f64 = 20.0;
const EKF_VELOCITY_RATIO: f64 = 0.35;
const EKF_MIN_VELOCITY_DEG_PER_SEC: f64 = 8.0;
const EKF_MAX_VELOCITY_DEG_PER_SEC: f64 = 120.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredAxisEkfState {
    position: f64,
    velocity: f64,
    bias: f64,
    covariance: [[f64; 3]; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredPtzEkfState {
    schema_version: u32,
    channel: u8,
    calibration_path: String,
    updated_at: u64,
    last_pan_u: f64,
    last_tilt_u: f64,
    pan: StoredAxisEkfState,
    tilt: StoredAxisEkfState,
}

struct ControlLoopConfig<'a> {
    target_pan_deg: f64,
    target_tilt_deg: f64,
    tolerance_deg: f64,
    deadline: Instant,
    params: &'a crate::core::model::CalibrationParams,
    calibration_path: &'a Path,
    ekf_state_path: PathBuf,
    timeout_ms: u64,
}

pub fn execute(
    client: &Client,
    channel: u8,
    target_pan_deg: f64,
    target_tilt_deg: f64,
    tolerance_deg: f64,
    timeout_ms: u64,
) -> AppResult<PtzAbsolutePosition> {
    validate_inputs(target_pan_deg, target_tilt_deg, tolerance_deg, timeout_ms)?;

    let operation_result = (|| {
        let (params, calibration_path) =
            ptz_calibrate_auto::load_or_create_params(client, channel)?;
        let _ = ptz_calibrate_auto::degrees_to_position(
            target_pan_deg,
            params.pan_offset,
            params.pan_scale,
        )?;
        let _ = ptz_calibrate_auto::degrees_to_position(
            target_tilt_deg,
            params.tilt_offset,
            params.tilt_scale,
        )?;
        let ekf_state_path = ekf_state_path_for_calibration(&calibration_path, channel);

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        run_closed_loop(
            client,
            channel,
            ControlLoopConfig {
                target_pan_deg,
                target_tilt_deg,
                tolerance_deg,
                deadline,
                params: &params,
                calibration_path: &calibration_path,
                ekf_state_path,
                timeout_ms,
            },
        )
    })();

    finalize_with_best_effort_stop(client, channel, operation_result)
}

fn run_closed_loop(
    client: &Client,
    channel: u8,
    config: ControlLoopConfig<'_>,
) -> AppResult<PtzAbsolutePosition> {
    let initial_status = ptz::get_ptz_cur_pos(client, channel)?;
    let (initial_pan_deg, initial_tilt_deg) =
        ptz_calibrate_auto::map_status_to_degrees(&initial_status, config.params)?;
    let status_with_ranges = ptz::get_ptz_status(client, channel).ok();
    let (pan_min_deg, pan_max_deg) = axis_degree_bounds(
        status_with_ranges
            .as_ref()
            .and_then(|status| status.pan_range.as_ref()),
        initial_pan_deg,
        config.params.pan_offset,
        config.params.pan_scale,
        DEFAULT_PAN_MIN_DEG,
        DEFAULT_PAN_MAX_DEG,
    );
    let (tilt_min_deg, tilt_max_deg) = axis_degree_bounds(
        status_with_ranges
            .as_ref()
            .and_then(|status| status.tilt_range.as_ref()),
        initial_tilt_deg,
        config.params.tilt_offset,
        config.params.tilt_scale,
        DEFAULT_TILT_MIN_DEG,
        DEFAULT_TILT_MAX_DEG,
    );

    let pan_model = sanitize_model(config.params.pan_model);
    let tilt_model = sanitize_model(config.params.tilt_model);
    let pan_ekf_config = ekf_config(pan_min_deg, pan_max_deg);
    let tilt_ekf_config = ekf_config(tilt_min_deg, tilt_max_deg);

    let mut pan_filter = AxisEkf::new(pan_ekf_config, pan_model, initial_pan_deg);
    let mut tilt_filter = AxisEkf::new(tilt_ekf_config, tilt_model, initial_tilt_deg);
    let mut last_pan_u = 0.0;
    let mut last_tilt_u = 0.0;
    let mut last_direction: Option<PtzDirection> = None;
    if let Some(stored) = load_stored_ekf_state(
        &config.ekf_state_path,
        channel,
        config.calibration_path.to_string_lossy().as_ref(),
    )? {
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
    }

    loop {
        let status = ptz::get_ptz_cur_pos(client, channel)?;
        let (current_pan_deg, current_tilt_deg) =
            ptz_calibrate_auto::map_status_to_degrees(&status, config.params)?;
        let pan_estimate = pan_filter.update(last_pan_u, current_pan_deg);
        let tilt_estimate = tilt_filter.update(last_tilt_u, current_tilt_deg);
        let estimated_pan_deg = pan_estimate.state.position + pan_estimate.state.bias;
        let estimated_tilt_deg = tilt_estimate.state.position + tilt_estimate.state.bias;

        let pan_error_deg = config.target_pan_deg - estimated_pan_deg;
        let tilt_error_deg = config.target_tilt_deg - estimated_tilt_deg;
        let pan_error_measured = config.target_pan_deg - current_pan_deg;
        let tilt_error_measured = config.target_tilt_deg - current_tilt_deg;
        let pan_error_control =
            select_control_error(pan_error_deg, pan_error_measured, config.tolerance_deg);
        let tilt_error_control =
            select_control_error(tilt_error_deg, tilt_error_measured, config.tolerance_deg);

        if pan_error_measured.abs() <= config.tolerance_deg
            && tilt_error_measured.abs() <= config.tolerance_deg
        {
            if last_direction.is_some() {
                ptz::stop_ptz(client, channel)?;
                last_direction = None;
                last_pan_u = 0.0;
                last_tilt_u = 0.0;
                save_stored_ekf_state(
                    &config.ekf_state_path,
                    channel,
                    config.calibration_path.to_string_lossy().as_ref(),
                    &pan_filter,
                    &tilt_filter,
                    last_pan_u,
                    last_tilt_u,
                )?;
                thread::sleep(Duration::from_millis(SETTLE_STEP_MS));
                continue;
            }
            save_stored_ekf_state(
                &config.ekf_state_path,
                channel,
                config.calibration_path.to_string_lossy().as_ref(),
                &pan_filter,
                &tilt_filter,
                0.0,
                0.0,
            )?;
            return Ok(PtzAbsolutePosition {
                channel,
                pan_deg: current_pan_deg,
                tilt_deg: current_tilt_deg,
                calibration_path: config.calibration_path.to_string_lossy().into_owned(),
            });
        }

        if Instant::now() >= config.deadline {
            let persist_error = save_stored_ekf_state(
                &config.ekf_state_path,
                channel,
                config.calibration_path.to_string_lossy().as_ref(),
                &pan_filter,
                &tilt_filter,
                last_pan_u,
                last_tilt_u,
            )
            .err();
            let persist_note = persist_error
                .as_ref()
                .map(|error| format!("; persist_error={}", error.message))
                .unwrap_or_default();
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "set_absolute timeout after {}ms on channel {channel}: target=({:.3},{:.3}) current=({current_pan_deg:.3},{current_tilt_deg:.3}) estimated=({estimated_pan_deg:.3},{estimated_tilt_deg:.3}) control_error=({pan_error_control:.3},{tilt_error_control:.3}) tolerance={:.3}; pan_state=(q={:.3},dq={:.3},b={:.3}) tilt_state=(q={:.3},dq={:.3},b={:.3}){}",
                    config.timeout_ms,
                    config.target_pan_deg,
                    config.target_tilt_deg,
                    config.tolerance_deg,
                    pan_estimate.state.position,
                    pan_estimate.state.velocity,
                    pan_estimate.state.bias,
                    tilt_estimate.state.position,
                    tilt_estimate.state.velocity,
                    tilt_estimate.state.bias,
                    persist_note,
                ),
            ));
        }

        match command_from_errors(pan_error_control, tilt_error_control, config.tolerance_deg) {
            Some((direction, speed)) => {
                if let Some(previous) = last_direction
                    && previous != direction
                {
                    ptz::stop_ptz(client, channel)?;
                }
                ptz::move_ptz(client, channel, direction, speed, None)?;
                last_direction = Some(direction);
                let (pan_u, tilt_u) = control_components_from_command(direction, speed);
                last_pan_u = pan_u;
                last_tilt_u = tilt_u;
            }
            None => {
                if last_direction.is_some() {
                    ptz::stop_ptz(client, channel)?;
                    last_direction = None;
                }
                last_pan_u = 0.0;
                last_tilt_u = 0.0;
            }
        }
        save_stored_ekf_state(
            &config.ekf_state_path,
            channel,
            config.calibration_path.to_string_lossy().as_ref(),
            &pan_filter,
            &tilt_filter,
            last_pan_u,
            last_tilt_u,
        )?;

        let control_step_ms = control_step_ms_for_error(pan_error_control, tilt_error_control);
        thread::sleep(Duration::from_millis(control_step_ms));
    }
}

fn speed_cap_from_error_deg(pan_error: f64, tilt_error: f64) -> u8 {
    let max_error = pan_error.abs().max(tilt_error.abs());
    if max_error <= 1.2 {
        2
    } else if max_error <= 2.5 {
        4
    } else if max_error <= 5.0 {
        8
    } else if max_error <= 12.0 {
        12
    } else if max_error <= 25.0 {
        20
    } else {
        32
    }
}

fn control_step_ms_for_error(pan_error: f64, tilt_error: f64) -> u64 {
    let max_error = pan_error.abs().max(tilt_error.abs());
    if max_error <= 1.5 {
        40
    } else if max_error <= 5.0 {
        50
    } else if max_error <= 15.0 {
        70
    } else {
        90
    }
}

fn command_from_errors(
    pan_error: f64,
    tilt_error: f64,
    tolerance_deg: f64,
) -> Option<(PtzDirection, u8)> {
    if pan_error.abs() <= tolerance_deg && tilt_error.abs() <= tolerance_deg {
        return None;
    }

    let speed = speed_cap_from_error_deg(pan_error, tilt_error).max(1);
    if pan_error.abs() >= tilt_error.abs() {
        if pan_error > 0.0 {
            Some((PtzDirection::Right, speed))
        } else if pan_error < 0.0 {
            Some((PtzDirection::Left, speed))
        } else if tilt_error > 0.0 {
            Some((PtzDirection::Up, speed))
        } else if tilt_error < 0.0 {
            Some((PtzDirection::Down, speed))
        } else {
            None
        }
    } else if tilt_error > 0.0 {
        Some((PtzDirection::Up, speed))
    } else if tilt_error < 0.0 {
        Some((PtzDirection::Down, speed))
    } else if pan_error > 0.0 {
        Some((PtzDirection::Right, speed))
    } else if pan_error < 0.0 {
        Some((PtzDirection::Left, speed))
    } else {
        None
    }
}

fn control_components_from_command(direction: PtzDirection, speed: u8) -> (f64, f64) {
    let normalized = (speed as f64 / 64.0).clamp(0.0, 1.0);
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

fn sanitize_model(model: AxisModelParams) -> AxisModelParams {
    let alpha = if model.alpha.is_finite() {
        model.alpha.clamp(0.5, 0.999)
    } else {
        0.9
    };
    let beta = if model.beta.is_finite() {
        model.beta.clamp(0.01, 1.0)
    } else {
        0.35
    };
    AxisModelParams { alpha, beta }
}

fn ekf_config(min_position: f64, max_position: f64) -> AxisEkfConfig {
    let mut config = AxisEkfConfig::with_default_noise(EKF_TS_SEC, min_position, max_position);
    let span = (max_position - min_position).abs().max(1.0);
    let velocity_limit = (span * EKF_VELOCITY_RATIO)
        .clamp(EKF_MIN_VELOCITY_DEG_PER_SEC, EKF_MAX_VELOCITY_DEG_PER_SEC);
    let bias_limit = (span * EKF_MAX_BIAS_RATIO).clamp(EKF_MIN_BIAS_DEG, EKF_MAX_BIAS_DEG);
    config.min_velocity = -velocity_limit;
    config.max_velocity = velocity_limit;
    config.min_bias = -bias_limit;
    config.max_bias = bias_limit;
    config
}

fn ekf_state_path_for_calibration(calibration_path: &Path, channel: u8) -> PathBuf {
    let stem = calibration_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("calibration");
    calibration_path.with_file_name(format!("{stem}.ch{channel}.ekf.json"))
}

fn load_stored_ekf_state(
    path: &Path,
    channel: u8,
    calibration_path: &str,
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
    if parsed.calibration_path != calibration_path {
        return Ok(None);
    }
    if !parsed.is_finite() {
        return Ok(None);
    }

    Ok(Some(parsed))
}

fn save_stored_ekf_state(
    path: &Path,
    channel: u8,
    calibration_path: &str,
    pan_filter: &AxisEkf,
    tilt_filter: &AxisEkf,
    last_pan_u: f64,
    last_tilt_u: f64,
) -> AppResult<()> {
    let stored = StoredPtzEkfState {
        schema_version: EKF_STATE_SCHEMA_VERSION,
        channel,
        calibration_path: calibration_path.to_string(),
        updated_at: now_epoch_millis(),
        last_pan_u: last_pan_u.clamp(-1.0, 1.0),
        last_tilt_u: last_tilt_u.clamp(-1.0, 1.0),
        pan: StoredAxisEkfState::from_snapshot(pan_filter.snapshot()),
        tilt: StoredAxisEkfState::from_snapshot(tilt_filter.snapshot()),
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

fn axis_degree_bounds(
    range: Option<&NumericRange>,
    current_deg: f64,
    offset: f64,
    scale: f64,
    fallback_min: f64,
    fallback_max: f64,
) -> (f64, f64) {
    let mut min_deg = fallback_min.min(fallback_max);
    let mut max_deg = fallback_max.max(fallback_min);

    if scale.is_finite()
        && scale.abs() > f64::EPSILON
        && offset.is_finite()
        && let Some(range) = range
    {
        let mapped_a = range.min as f64 * scale + offset;
        let mapped_b = range.max as f64 * scale + offset;
        if mapped_a.is_finite() && mapped_b.is_finite() {
            min_deg = mapped_a.min(mapped_b);
            max_deg = mapped_a.max(mapped_b);
        }
    }

    min_deg = min_deg.min(current_deg) - EKF_POSITION_MARGIN_DEG;
    max_deg = max_deg.max(current_deg) + EKF_POSITION_MARGIN_DEG;
    if max_deg <= min_deg {
        return (
            fallback_min.min(fallback_max),
            fallback_max.max(fallback_min),
        );
    }
    (min_deg, max_deg)
}

fn select_control_error(estimated_error: f64, measured_error: f64, tolerance_deg: f64) -> f64 {
    if !estimated_error.is_finite() {
        return measured_error;
    }
    if measured_error.abs() <= tolerance_deg {
        return measured_error;
    }
    if estimated_error.signum() != measured_error.signum() {
        return measured_error;
    }

    let disagreement = (estimated_error - measured_error).abs();
    let allowed = measured_error.abs().max(1.0) * 1.5;
    if disagreement > allowed {
        measured_error
    } else {
        estimated_error
    }
}

fn finalize_with_best_effort_stop<T>(
    client: &Client,
    channel: u8,
    result: AppResult<T>,
) -> AppResult<T> {
    let stop_error = ptz::stop_ptz(client, channel).err();

    match result {
        Ok(value) => {
            if let Some(error) = stop_error {
                return Err(AppError::new(
                    ErrorKind::UnexpectedResponse,
                    format!(
                        "set_absolute completed but failed to send Stop on channel {channel}: {}",
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

fn validate_inputs(
    target_pan_deg: f64,
    target_tilt_deg: f64,
    tolerance_deg: f64,
    timeout_ms: u64,
) -> AppResult<()> {
    if !target_pan_deg.is_finite() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "target pan degree must be a finite number",
        ));
    }
    if !target_tilt_deg.is_finite() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "target tilt degree must be a finite number",
        ));
    }
    if !tolerance_deg.is_finite() || tolerance_deg <= 0.0 {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "tolerance degree must be a finite value > 0",
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
        }
    }

    fn is_finite(&self) -> bool {
        self.position.is_finite()
            && self.velocity.is_finite()
            && self.bias.is_finite()
            && self
                .covariance
                .iter()
                .all(|row| row.iter().all(|value| value.is_finite()))
    }
}

impl StoredPtzEkfState {
    fn is_finite(&self) -> bool {
        self.last_pan_u.is_finite()
            && self.last_tilt_u.is_finite()
            && self.pan.is_finite()
            && self.tilt.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        axis_degree_bounds, ekf_config, ekf_state_path_for_calibration, load_stored_ekf_state,
        save_stored_ekf_state, select_control_error,
    };
    use crate::app::usecases::ptz_controller::AxisEkf;
    use crate::core::model::{AxisModelParams, NumericRange};

    #[test]
    fn select_control_error_uses_measured_when_sign_conflicts() {
        let chosen = select_control_error(8.0, -4.0, 1.0);
        assert_eq!(chosen, -4.0);
    }

    #[test]
    fn select_control_error_uses_estimated_when_consistent() {
        let chosen = select_control_error(-6.0, -4.5, 1.0);
        assert_eq!(chosen, -6.0);
    }

    #[test]
    fn select_control_error_uses_measured_when_estimate_is_invalid() {
        let chosen = select_control_error(f64::NAN, 2.0, 1.0);
        assert_eq!(chosen, 2.0);
    }

    #[test]
    fn axis_degree_bounds_maps_range_and_applies_margin() {
        let range = NumericRange {
            min: 1000,
            max: 2000,
        };
        let (min_deg, max_deg) = axis_degree_bounds(Some(&range), 15.0, -100.0, 0.1, -220.0, 220.0);
        assert!((min_deg - -5.0).abs() < 1e-6);
        assert!((max_deg - 105.0).abs() < 1e-6);
    }

    #[test]
    fn ekf_state_path_uses_channel_suffix() {
        let path = PathBuf::from("/tmp/camera-key.json");
        let state_path = ekf_state_path_for_calibration(&path, 3);
        assert_eq!(state_path, PathBuf::from("/tmp/camera-key.ch3.ekf.json"));
    }

    #[test]
    fn ekf_state_roundtrip_save_and_load() {
        let temp_file = std::env::temp_dir().join(format!(
            "reocli-ekf-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let channel = 1u8;
        let calibration_path = "/tmp/calibration-A.json";
        let model = AxisModelParams {
            alpha: 0.9,
            beta: 0.4,
        };
        let mut pan_filter = AxisEkf::new(ekf_config(-180.0, 180.0), model, 12.0);
        let mut tilt_filter = AxisEkf::new(ekf_config(-90.0, 90.0), model, -3.0);
        let _ = pan_filter.update(0.2, 13.0);
        let _ = tilt_filter.update(-0.3, -3.5);

        save_stored_ekf_state(
            &temp_file,
            channel,
            calibration_path,
            &pan_filter,
            &tilt_filter,
            0.25,
            -0.5,
        )
        .expect("EKF state save should succeed");

        let loaded = load_stored_ekf_state(&temp_file, channel, calibration_path)
            .expect("EKF state load should succeed")
            .expect("EKF state should exist");
        assert_eq!(loaded.channel, channel);
        assert_eq!(loaded.calibration_path, calibration_path);
        assert!((loaded.last_pan_u - 0.25).abs() < 1e-9);
        assert!((loaded.last_tilt_u + 0.5).abs() < 1e-9);
        assert!(loaded.pan.position.is_finite());
        assert!(loaded.tilt.position.is_finite());

        let _ = fs::remove_file(&temp_file);
    }
}
