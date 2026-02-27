use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{
    AxisModelParams, CalibrationParams, CalibrationReport, DeviceInfo, NumericRange, PtzStatus,
};
use crate::interfaces::runtime;
use crate::reolink::client::Client;
use crate::reolink::{device, ptz};

use super::ptz_transport;

const CALIBRATION_SCHEMA_VERSION: u32 = 2;
const CALIBRATION_SOURCE_HEURISTIC: &str = "auto_heuristic";
const CALIBRATION_SOURCE_MEASURED: &str = "auto_measured";
const DEFAULT_PAN_SPAN_UNITS: i64 = 3600;
const DEFAULT_TILT_SPAN_UNITS: i64 = 1800;
const DEFAULT_MODEL_ALPHA: f64 = 0.9;
const DEFAULT_MODEL_BETA_RATIO: f64 = 0.03;
const MODEL_ALPHA_MIN: f64 = 0.75;
const MODEL_ALPHA_MAX: f64 = 0.98;
const MODEL_BETA_MIN: f64 = 20.0;
const MODEL_BETA_MAX: f64 = 600.0;
const CALIBRATION_PULSE_SPEED: u8 = 6;
const CALIBRATION_PULSE_MS: u64 = 220;
const CALIBRATION_SETTLE_MS: u64 = 80;
const CALIBRATION_MIN_MOVE_DELTA: i64 = 8;
const CALIBRATION_STALL_DELTA: i64 = 4;
const CALIBRATION_STALL_STEPS: usize = 3;
const CALIBRATION_SWEEP_MAX_STEPS: usize = 48;
const CALIBRATION_RESTORE_MAX_STEPS: usize = 36;
const CALIBRATION_RESTORE_TOLERANCE: i64 = 10;
const DEFAULT_DEADBAND_COUNT: i64 = 6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredCalibration {
    pub schema_version: u32,
    pub source: String,
    pub camera_key: String,
    pub channel: u8,
    pub calibration: CalibrationParams,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PtzCalibrationReport {
    pub channel: u8,
    pub camera_key: String,
    pub calibration_path: String,
    pub reused_existing: bool,
    pub calibrated_state: Option<bool>,
    pub pan_count: i64,
    pub tilt_count: i64,
    pub params: StoredCalibration,
    pub calibration: CalibrationParams,
    pub report: CalibrationReport,
}

pub fn execute(client: &Client, channel: u8) -> AppResult<PtzCalibrationReport> {
    let device_info = device::get_dev_info(client)?;
    let status = ptz::get_ptz_status(client, channel)?;
    let calibration_path = runtime::calibration_file_path_for_camera(&device_info);

    if status.calibrated() == Some(true)
        && let Some((saved_params, _)) = load_saved_params_for_device(&device_info)?
    {
        let (pan_count, tilt_count) = map_status_to_counts(&status)?;
        return Ok(PtzCalibrationReport {
            channel,
            camera_key: saved_params.camera_key.clone(),
            calibration_path: calibration_path.to_string_lossy().into_owned(),
            reused_existing: true,
            calibrated_state: status.calibrated(),
            pan_count,
            tilt_count,
            params: saved_params.clone(),
            calibration: saved_params.calibration,
            report: CalibrationReport {
                samples: 1,
                pan_error_p95_count: 0,
                tilt_error_p95_count: 0,
                notes: "reused_saved_params".to_string(),
            },
        });
    }

    let (calibration, report, source) =
        match try_build_measured_calibration(client, channel, &device_info, &status) {
            Ok(calibration) => (
                calibration,
                CalibrationReport {
                    samples: 8,
                    pan_error_p95_count: 0,
                    tilt_error_p95_count: 0,
                    notes: "created_from_measured_sweep".to_string(),
                },
                CALIBRATION_SOURCE_MEASURED,
            ),
            Err(error) => (
                build_heuristic_calibration(&device_info, &status),
                CalibrationReport {
                    samples: 1,
                    pan_error_p95_count: 0,
                    tilt_error_p95_count: 0,
                    notes: format!("fallback_to_heuristic: {}", error.message),
                },
                CALIBRATION_SOURCE_HEURISTIC,
            ),
        };

    let stored = StoredCalibration {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        source: source.to_string(),
        camera_key: runtime::calibration_camera_key(&device_info),
        channel,
        calibration,
    };

    save_stored_calibration(&calibration_path, &stored)?;
    let final_status = ptz::get_ptz_cur_pos(client, channel).unwrap_or(status.clone());
    let (pan_count, tilt_count) = map_status_to_counts(&final_status)?;

    Ok(PtzCalibrationReport {
        channel,
        camera_key: stored.camera_key.clone(),
        calibration_path: calibration_path.to_string_lossy().into_owned(),
        reused_existing: false,
        calibrated_state: final_status.calibrated(),
        pan_count,
        tilt_count,
        params: stored.clone(),
        calibration: stored.calibration,
        report,
    })
}

pub(crate) fn load_saved_params_for_device(
    device_info: &DeviceInfo,
) -> AppResult<Option<(StoredCalibration, PathBuf)>> {
    let calibration_path = runtime::calibration_file_path_for_camera(device_info);
    let expected_camera_key = runtime::calibration_camera_key(device_info);

    let raw = match fs::read_to_string(&calibration_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "failed to read calibration file {}: {error}",
                    calibration_path.display()
                ),
            ));
        }
    };

    let stored = match serde_json::from_str::<StoredCalibration>(&raw) {
        Ok(stored) => stored,
        Err(_) => return Ok(None),
    };

    if stored.schema_version != CALIBRATION_SCHEMA_VERSION {
        return Ok(None);
    }

    if stored.camera_key != expected_camera_key {
        return Ok(None);
    }

    Ok(Some((stored, calibration_path)))
}

pub(crate) fn map_status_to_counts(status: &PtzStatus) -> AppResult<(i64, i64)> {
    let pan_position = status.pan_position.ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "PTZ status missing pan position for channel {}",
                status.channel
            ),
        )
    })?;

    let tilt_position = status.tilt_position.ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "PTZ status missing tilt position for channel {}",
                status.channel
            ),
        )
    })?;
    Ok((pan_position, tilt_position))
}

#[derive(Debug, Clone, Copy)]
enum AxisKind {
    Pan,
    Tilt,
}

#[derive(Debug, Clone, Copy)]
struct AxisMotion {
    increase: crate::core::model::PtzDirection,
    decrease: crate::core::model::PtzDirection,
}

fn try_build_measured_calibration(
    client: &Client,
    channel: u8,
    device_info: &DeviceInfo,
    status: &PtzStatus,
) -> AppResult<CalibrationParams> {
    let measured = (|| {
        let home_pan = axis_position(status, AxisKind::Pan)?;
        let home_tilt = axis_position(status, AxisKind::Tilt)?;

        let pan_motion = detect_axis_motion(client, channel, AxisKind::Pan)?;
        let tilt_motion = detect_axis_motion(client, channel, AxisKind::Tilt)?;

        let (pan_min, pan_max, pan_sweep_deltas) =
            sweep_axis_bounds(client, channel, AxisKind::Pan, pan_motion)?;
        let (tilt_min, tilt_max, tilt_sweep_deltas) =
            sweep_axis_bounds(client, channel, AxisKind::Tilt, tilt_motion)?;

        restore_axis_to_home(client, channel, AxisKind::Pan, home_pan, pan_motion)?;
        restore_axis_to_home(client, channel, AxisKind::Tilt, home_tilt, tilt_motion)?;
        let pan_span = (pan_max - pan_min).unsigned_abs() as f64;
        let tilt_span = (tilt_max - tilt_min).unsigned_abs() as f64;
        let pan_model = estimate_model_from_sweep(pan_span, &pan_sweep_deltas);
        let tilt_model = estimate_model_from_sweep(tilt_span, &tilt_sweep_deltas);
        let pan_deadband_count =
            estimate_deadband_count(client, channel, AxisKind::Pan, pan_motion)
                .unwrap_or(DEFAULT_DEADBAND_COUNT);
        let tilt_deadband_count =
            estimate_deadband_count(client, channel, AxisKind::Tilt, tilt_motion)
                .unwrap_or(DEFAULT_DEADBAND_COUNT);

        Ok(CalibrationParams {
            serial_number: device_info.serial_number.clone(),
            model: device_info.model.clone(),
            firmware: device_info.firmware.clone(),
            pan_min_count: pan_min,
            pan_max_count: pan_max,
            pan_deadband_count,
            tilt_min_count: tilt_min,
            tilt_max_count: tilt_max,
            tilt_deadband_count,
            pan_model,
            tilt_model,
            created_at: now_epoch_millis().to_string(),
        })
    })();

    let _ = ptz_transport::stop_ptz(client, channel);
    measured
}

fn detect_axis_motion(client: &Client, channel: u8, axis: AxisKind) -> AppResult<AxisMotion> {
    let (primary, secondary) = match axis {
        AxisKind::Pan => (
            crate::core::model::PtzDirection::Right,
            crate::core::model::PtzDirection::Left,
        ),
        AxisKind::Tilt => (
            crate::core::model::PtzDirection::Up,
            crate::core::model::PtzDirection::Down,
        ),
    };

    let before_primary = read_axis_position(client, channel, axis)?;
    pulse(client, channel, primary)?;
    let after_primary = read_axis_position(client, channel, axis)?;
    let primary_delta = after_primary - before_primary;
    pulse(client, channel, secondary)?;

    if primary_delta.abs() >= CALIBRATION_MIN_MOVE_DELTA {
        return if primary_delta > 0 {
            Ok(AxisMotion {
                increase: primary,
                decrease: secondary,
            })
        } else {
            Ok(AxisMotion {
                increase: secondary,
                decrease: primary,
            })
        };
    }

    let before_secondary = read_axis_position(client, channel, axis)?;
    pulse(client, channel, secondary)?;
    let after_secondary = read_axis_position(client, channel, axis)?;
    let secondary_delta = after_secondary - before_secondary;
    pulse(client, channel, primary)?;

    if secondary_delta.abs() < CALIBRATION_MIN_MOVE_DELTA {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "failed to detect {:?} axis direction: movement delta too small ({primary_delta}, {secondary_delta})",
                axis
            ),
        ));
    }

    if secondary_delta > 0 {
        Ok(AxisMotion {
            increase: secondary,
            decrease: primary,
        })
    } else {
        Ok(AxisMotion {
            increase: primary,
            decrease: secondary,
        })
    }
}

fn sweep_axis_bounds(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    motion: AxisMotion,
) -> AppResult<(i64, i64, Vec<f64>)> {
    let mut sweep_deltas = Vec::new();
    let min = sweep_axis_limit(
        client,
        channel,
        axis,
        motion.decrease,
        true,
        &mut sweep_deltas,
    )?;
    let max = sweep_axis_limit(
        client,
        channel,
        axis,
        motion.increase,
        false,
        &mut sweep_deltas,
    )?;
    if max <= min {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "invalid {:?} span from measured sweep: min={min}, max={max}",
                axis
            ),
        ));
    }

    Ok((min, max, sweep_deltas))
}

fn sweep_axis_limit(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    direction: crate::core::model::PtzDirection,
    toward_min: bool,
    sweep_deltas: &mut Vec<f64>,
) -> AppResult<i64> {
    let mut best = read_axis_position(client, channel, axis)?;
    let mut stall_steps = 0usize;

    for _ in 0..CALIBRATION_SWEEP_MAX_STEPS {
        let before = read_axis_position(client, channel, axis)?;
        pulse(client, channel, direction)?;
        let after = read_axis_position(client, channel, axis)?;
        let delta_signed = after - before;
        let moved_toward_target = if toward_min {
            -(delta_signed as f64)
        } else {
            delta_signed as f64
        };
        if moved_toward_target.is_finite() && moved_toward_target > 0.0 {
            sweep_deltas.push(moved_toward_target);
        }

        best = if toward_min {
            best.min(after)
        } else {
            best.max(after)
        };

        if (after - before).abs() <= CALIBRATION_STALL_DELTA {
            stall_steps += 1;
            if stall_steps >= CALIBRATION_STALL_STEPS {
                break;
            }
        } else {
            stall_steps = 0;
        }
    }

    Ok(best)
}

fn restore_axis_to_home(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    home_position: i64,
    motion: AxisMotion,
) -> AppResult<()> {
    for _ in 0..CALIBRATION_RESTORE_MAX_STEPS {
        let current = read_axis_position(client, channel, axis)?;
        let error = home_position - current;
        if error.abs() <= CALIBRATION_RESTORE_TOLERANCE {
            return Ok(());
        }

        let direction = if error > 0 {
            motion.increase
        } else {
            motion.decrease
        };
        let speed = if error.abs() <= 40 {
            2
        } else if error.abs() <= 140 {
            4
        } else {
            CALIBRATION_PULSE_SPEED
        };

        ptz_transport::move_ptz(
            client,
            channel,
            direction,
            speed,
            Some(CALIBRATION_PULSE_MS),
        )?;
        thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    }

    Ok(())
}

fn estimate_deadband_count(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    motion: AxisMotion,
) -> AppResult<i64> {
    let before = read_axis_position(client, channel, axis)?;
    ptz_transport::move_ptz(client, channel, motion.increase, 2, Some(80))?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    let after_increase = read_axis_position(client, channel, axis)?;
    ptz_transport::move_ptz(client, channel, motion.decrease, 2, Some(80))?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    let after_decrease = read_axis_position(client, channel, axis)?;

    let delta_units = (after_increase - before)
        .abs()
        .max((after_decrease - after_increase).abs())
        .max(1);
    Ok(delta_units.max(1))
}

fn pulse(
    client: &Client,
    channel: u8,
    direction: crate::core::model::PtzDirection,
) -> AppResult<()> {
    ptz_transport::move_ptz(
        client,
        channel,
        direction,
        CALIBRATION_PULSE_SPEED,
        Some(CALIBRATION_PULSE_MS),
    )?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    Ok(())
}

fn read_axis_position(client: &Client, channel: u8, axis: AxisKind) -> AppResult<i64> {
    let status = ptz::get_ptz_cur_pos(client, channel)?;
    axis_position(&status, axis)
}

fn axis_position(status: &PtzStatus, axis: AxisKind) -> AppResult<i64> {
    match axis {
        AxisKind::Pan => status.pan_position.ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "PTZ status missing pan position for channel {}",
                    status.channel
                ),
            )
        }),
        AxisKind::Tilt => status.tilt_position.ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "PTZ status missing tilt position for channel {}",
                    status.channel
                ),
            )
        }),
    }
}

fn build_heuristic_calibration(device_info: &DeviceInfo, status: &PtzStatus) -> CalibrationParams {
    let pan_range = build_axis_count_range(
        status.pan_range.as_ref(),
        status.pan_position,
        DEFAULT_PAN_SPAN_UNITS,
    );
    let tilt_range = build_axis_count_range(
        status.tilt_range.as_ref(),
        status.tilt_position,
        DEFAULT_TILT_SPAN_UNITS,
    );

    let pan_span = (pan_range.max_count - pan_range.min_count).unsigned_abs() as f64;
    let tilt_span = (tilt_range.max_count - tilt_range.min_count).unsigned_abs() as f64;
    CalibrationParams {
        serial_number: device_info.serial_number.clone(),
        model: device_info.model.clone(),
        firmware: device_info.firmware.clone(),
        pan_min_count: pan_range.min_count,
        pan_max_count: pan_range.max_count,
        pan_deadband_count: DEFAULT_DEADBAND_COUNT,
        tilt_min_count: tilt_range.min_count,
        tilt_max_count: tilt_range.max_count,
        tilt_deadband_count: DEFAULT_DEADBAND_COUNT,
        pan_model: fallback_model_for_span(pan_span),
        tilt_model: fallback_model_for_span(tilt_span),
        created_at: now_epoch_millis().to_string(),
    }
}

#[derive(Debug, Clone, Copy)]
struct AxisCountRange {
    min_count: i64,
    max_count: i64,
}

fn build_axis_count_range(
    range: Option<&NumericRange>,
    current_position: Option<i64>,
    default_span_count: i64,
) -> AxisCountRange {
    let (mut pos_min, mut pos_max) = match range {
        Some(range) if range.max > range.min => (range.min, range.max),
        Some(range) => (range.min, range.min + 1),
        None => {
            let center = current_position.unwrap_or(0);
            let half_span = (default_span_count / 2).max(1);
            (center - half_span, center + half_span)
        }
    };

    if let Some(position) = current_position {
        pos_min = pos_min.min(position);
        pos_max = pos_max.max(position);
    }

    if pos_max <= pos_min {
        pos_max = pos_min + 1;
    }

    AxisCountRange {
        min_count: pos_min,
        max_count: pos_max,
    }
}

fn estimate_model_from_sweep(span: f64, sweep_deltas: &[f64]) -> AxisModelParams {
    let fallback = fallback_model_for_span(span);
    let samples = sweep_deltas
        .iter()
        .copied()
        .filter(|delta| delta.is_finite() && *delta > 0.0)
        .collect::<Vec<_>>();
    if samples.len() < 2 {
        return fallback;
    }

    let mut alpha_numer = 0.0f64;
    let mut alpha_denom = 0.0f64;
    for window in samples.windows(2) {
        let prev = window[0];
        let next = window[1];
        alpha_numer += prev * next;
        alpha_denom += prev * prev;
    }
    if alpha_denom <= f64::EPSILON {
        return fallback;
    }

    let alpha = (alpha_numer / alpha_denom).clamp(MODEL_ALPHA_MIN, MODEL_ALPHA_MAX);
    let mean_delta = samples.iter().sum::<f64>() / samples.len() as f64;
    let velocity = mean_delta / calibration_effective_ts_sec();
    let beta =
        (velocity * (1.0 - alpha) / calibration_control_u()).clamp(MODEL_BETA_MIN, MODEL_BETA_MAX);
    if !alpha.is_finite() || !beta.is_finite() {
        return fallback;
    }

    AxisModelParams { alpha, beta }
}

fn fallback_model_for_span(span: f64) -> AxisModelParams {
    AxisModelParams {
        alpha: DEFAULT_MODEL_ALPHA,
        beta: (span.abs().max(1.0) * DEFAULT_MODEL_BETA_RATIO)
            .clamp(MODEL_BETA_MIN, MODEL_BETA_MAX),
    }
}

fn calibration_control_u() -> f64 {
    let speed_factor = (CALIBRATION_PULSE_SPEED as f64 / 64.0).clamp(0.0, 1.0);
    let pulse_factor = (CALIBRATION_PULSE_MS as f64 / 120.0).clamp(0.5, 1.5);
    (speed_factor * pulse_factor).clamp(1e-3, 1.0)
}

fn calibration_effective_ts_sec() -> f64 {
    ((CALIBRATION_PULSE_MS + CALIBRATION_SETTLE_MS) as f64 / 1_000.0).clamp(0.05, 0.5)
}

fn save_stored_calibration(path: &Path, stored: &StoredCalibration) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "failed to create calibration directory {}: {error}",
                    parent.display()
                ),
            )
        })?;
    }

    let serialized = serde_json::to_string_pretty(stored).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to serialize PTZ calibration JSON: {error}"),
        )
    })?;

    fs::write(path, serialized).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "failed to write calibration file {}: {error}",
                path.display()
            ),
        )
    })
}

fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{build_axis_count_range, estimate_model_from_sweep, map_status_to_counts};
    use crate::core::model::{NumericRange, PtzStatus};

    #[test]
    fn build_axis_count_range_uses_range_when_present() {
        let range = NumericRange {
            min: -3550,
            max: 3550,
        };
        let mapped = build_axis_count_range(Some(&range), Some(120), 3600);
        assert_eq!(mapped.min_count, -3550);
        assert_eq!(mapped.max_count, 3550);
    }

    #[test]
    fn build_axis_count_range_falls_back_to_default_span() {
        let mapped = build_axis_count_range(None, Some(500), 2000);
        assert_eq!(mapped.min_count, -500);
        assert_eq!(mapped.max_count, 1500);
    }

    #[test]
    fn map_status_to_counts_returns_raw_positions() {
        let status = PtzStatus {
            channel: 2,
            pan_position: Some(1500),
            tilt_position: Some(-180),
            ..PtzStatus::default()
        };
        let (pan_count, tilt_count) = map_status_to_counts(&status).expect("status should map");
        assert_eq!(pan_count, 1500);
        assert_eq!(tilt_count, -180);
    }

    #[test]
    fn estimate_model_from_sweep_uses_fallback_for_insufficient_samples() {
        let model = estimate_model_from_sweep(7_200.0, &[8.0]);
        assert!((0.75..=0.98).contains(&model.alpha));
        assert!((20.0..=600.0).contains(&model.beta));
    }

    #[test]
    fn estimate_model_from_sweep_produces_finite_model() {
        let model = estimate_model_from_sweep(7_200.0, &[80.0, 97.0, 108.0, 114.0, 118.0]);
        assert!(model.alpha.is_finite());
        assert!(model.beta.is_finite());
        assert!((0.75..=0.98).contains(&model.alpha));
        assert!((20.0..=600.0).contains(&model.beta));
    }
}
