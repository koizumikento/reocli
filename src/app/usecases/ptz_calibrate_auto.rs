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

const CALIBRATION_SCHEMA_VERSION: u32 = 1;
const CALIBRATION_SOURCE_HEURISTIC: &str = "auto_heuristic";
const CALIBRATION_SOURCE_MEASURED: &str = "auto_measured";
const DEFAULT_PAN_SPAN_UNITS: i64 = 3600;
const DEFAULT_TILT_SPAN_UNITS: i64 = 1800;
const DEFAULT_PAN_MIN_DEG: f64 = -180.0;
const DEFAULT_PAN_MAX_DEG: f64 = 180.0;
const DEFAULT_TILT_MIN_DEG: f64 = -90.0;
const DEFAULT_TILT_MAX_DEG: f64 = 90.0;
const DEFAULT_UPWARD_TILT_MIN_DEG: f64 = 0.0;
const DEFAULT_UPWARD_TILT_MAX_DEG: f64 = 90.0;
const DEFAULT_MODEL_ALPHA: f64 = 0.9;
const DEFAULT_MODEL_BETA: f64 = 0.4;
const CALIBRATION_PULSE_SPEED: u8 = 6;
const CALIBRATION_PULSE_MS: u64 = 220;
const CALIBRATION_SETTLE_MS: u64 = 80;
const CALIBRATION_MIN_MOVE_DELTA: i64 = 8;
const CALIBRATION_STALL_DELTA: i64 = 4;
const CALIBRATION_STALL_STEPS: usize = 3;
const CALIBRATION_SWEEP_MAX_STEPS: usize = 48;
const CALIBRATION_RESTORE_MAX_STEPS: usize = 36;
const CALIBRATION_RESTORE_TOLERANCE: i64 = 10;

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
    pub pan_deg: f64,
    pub tilt_deg: f64,
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
        let (pan_deg, tilt_deg) = map_status_to_degrees(&status, &saved_params.calibration)?;
        return Ok(PtzCalibrationReport {
            channel,
            camera_key: saved_params.camera_key.clone(),
            calibration_path: calibration_path.to_string_lossy().into_owned(),
            reused_existing: true,
            calibrated_state: status.calibrated(),
            pan_deg,
            tilt_deg,
            params: saved_params.clone(),
            calibration: saved_params.calibration,
            report: CalibrationReport {
                samples: 1,
                pan_error_p95_deg: 0.0,
                tilt_error_p95_deg: 0.0,
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
                    pan_error_p95_deg: 0.0,
                    tilt_error_p95_deg: 0.0,
                    notes: "created_from_measured_sweep".to_string(),
                },
                CALIBRATION_SOURCE_MEASURED,
            ),
            Err(error) => (
                build_heuristic_calibration(&device_info, &status),
                CalibrationReport {
                    samples: 1,
                    pan_error_p95_deg: 0.0,
                    tilt_error_p95_deg: 0.0,
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
    let (pan_deg, tilt_deg) = map_status_to_degrees(&final_status, &stored.calibration)?;

    Ok(PtzCalibrationReport {
        channel,
        camera_key: stored.camera_key.clone(),
        calibration_path: calibration_path.to_string_lossy().into_owned(),
        reused_existing: false,
        calibrated_state: final_status.calibrated(),
        pan_deg,
        tilt_deg,
        params: stored.clone(),
        calibration: stored.calibration,
        report,
    })
}

pub(crate) fn load_or_create_params(
    client: &Client,
    channel: u8,
) -> AppResult<(CalibrationParams, PathBuf)> {
    let device_info = device::get_dev_info(client)?;
    if let Some((saved_params, path)) = load_saved_params_for_device(&device_info)? {
        return Ok((saved_params.calibration, path));
    }

    let report = execute(client, channel)?;
    Ok((report.calibration, PathBuf::from(report.calibration_path)))
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

pub(crate) fn map_status_to_degrees(
    status: &PtzStatus,
    calibration: &CalibrationParams,
) -> AppResult<(f64, f64)> {
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

    let pan_deg = position_to_degrees(pan_position, calibration.pan_offset, calibration.pan_scale)?;
    let tilt_deg = position_to_degrees(
        tilt_position,
        calibration.tilt_offset,
        calibration.tilt_scale,
    )?;
    Ok((pan_deg, tilt_deg))
}

pub(crate) fn degrees_to_position(degrees: f64, offset: f64, scale: f64) -> AppResult<i64> {
    if !degrees.is_finite() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "target degree must be a finite number",
        ));
    }

    ensure_valid_linear_mapping(offset, scale)?;
    let mapped = (degrees - offset) / scale;
    Ok(mapped.round() as i64)
}

fn position_to_degrees(position: i64, offset: f64, scale: f64) -> AppResult<f64> {
    ensure_valid_linear_mapping(offset, scale)?;
    Ok(position as f64 * scale + offset)
}

fn ensure_valid_linear_mapping(offset: f64, scale: f64) -> AppResult<()> {
    if !offset.is_finite() || !scale.is_finite() || scale.abs() <= f64::EPSILON {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "invalid calibration mapping: offset={offset} scale={scale}; scale must be finite and non-zero"
            ),
        ));
    }
    Ok(())
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

        let (pan_min, pan_max) = sweep_axis_bounds(client, channel, AxisKind::Pan, pan_motion)?;
        let (tilt_min, tilt_max) = sweep_axis_bounds(client, channel, AxisKind::Tilt, tilt_motion)?;

        restore_axis_to_home(client, channel, AxisKind::Pan, home_pan, pan_motion)?;
        restore_axis_to_home(client, channel, AxisKind::Tilt, home_tilt, tilt_motion)?;
        let reference_pan = read_axis_position(client, channel, AxisKind::Pan).unwrap_or(home_pan);
        let reference_tilt =
            read_axis_position(client, channel, AxisKind::Tilt).unwrap_or(home_tilt);

        let pan_span = (pan_max - pan_min).abs().max(1) as f64;
        let pan_scale_magnitude = (DEFAULT_PAN_MAX_DEG - DEFAULT_PAN_MIN_DEG) / pan_span;
        let pan_sign = if pan_motion.increase == crate::core::model::PtzDirection::Right {
            1.0
        } else {
            -1.0
        };
        let pan_scale = pan_scale_magnitude * pan_sign;
        let pan_offset = -(reference_pan as f64) * pan_scale;

        let tilt_span = (tilt_max - tilt_min).abs().max(1) as f64;
        let tilt_deg_span = if tilt_min >= 0 {
            DEFAULT_UPWARD_TILT_MAX_DEG - DEFAULT_UPWARD_TILT_MIN_DEG
        } else {
            DEFAULT_TILT_MAX_DEG - DEFAULT_TILT_MIN_DEG
        };
        let tilt_scale_magnitude = tilt_deg_span / tilt_span;
        let tilt_sign = if tilt_motion.increase == crate::core::model::PtzDirection::Up {
            1.0
        } else {
            -1.0
        };
        let tilt_scale = tilt_scale_magnitude * tilt_sign;
        let tilt_offset = -(reference_tilt as f64) * tilt_scale;

        let pan_deadband = estimate_deadband(client, channel, AxisKind::Pan, pan_motion, pan_scale)
            .unwrap_or_else(|_| pan_scale.abs().max(0.05));
        let tilt_deadband =
            estimate_deadband(client, channel, AxisKind::Tilt, tilt_motion, tilt_scale)
                .unwrap_or_else(|_| tilt_scale.abs().max(0.05));

        Ok(CalibrationParams {
            serial_number: device_info.serial_number.clone(),
            model: device_info.model.clone(),
            firmware: device_info.firmware.clone(),
            pan_offset,
            pan_scale,
            pan_deadband,
            tilt_offset,
            tilt_scale,
            tilt_deadband,
            pan_model: AxisModelParams {
                alpha: DEFAULT_MODEL_ALPHA,
                beta: DEFAULT_MODEL_BETA,
            },
            tilt_model: AxisModelParams {
                alpha: DEFAULT_MODEL_ALPHA,
                beta: DEFAULT_MODEL_BETA,
            },
            created_at: now_epoch_millis().to_string(),
        })
    })();

    let _ = ptz::stop_ptz(client, channel);
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
) -> AppResult<(i64, i64)> {
    let min = sweep_axis_limit(client, channel, axis, motion.decrease, true)?;
    let max = sweep_axis_limit(client, channel, axis, motion.increase, false)?;
    if max <= min {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "invalid {:?} span from measured sweep: min={min}, max={max}",
                axis
            ),
        ));
    }

    Ok((min, max))
}

fn sweep_axis_limit(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    direction: crate::core::model::PtzDirection,
    toward_min: bool,
) -> AppResult<i64> {
    let mut best = read_axis_position(client, channel, axis)?;
    let mut stall_steps = 0usize;

    for _ in 0..CALIBRATION_SWEEP_MAX_STEPS {
        let before = read_axis_position(client, channel, axis)?;
        pulse(client, channel, direction)?;
        let after = read_axis_position(client, channel, axis)?;

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

        ptz::move_ptz(
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

fn estimate_deadband(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    motion: AxisMotion,
    scale_deg_per_unit: f64,
) -> AppResult<f64> {
    let before = read_axis_position(client, channel, axis)?;
    ptz::move_ptz(client, channel, motion.increase, 2, Some(80))?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    let after_increase = read_axis_position(client, channel, axis)?;
    ptz::move_ptz(client, channel, motion.decrease, 2, Some(80))?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    let after_decrease = read_axis_position(client, channel, axis)?;

    let delta_units = (after_increase - before)
        .abs()
        .max((after_decrease - after_increase).abs())
        .max(1);
    let deadband = (delta_units as f64 * scale_deg_per_unit.abs()).clamp(0.05, 2.0);
    Ok(deadband)
}

fn pulse(
    client: &Client,
    channel: u8,
    direction: crate::core::model::PtzDirection,
) -> AppResult<()> {
    ptz::move_ptz(
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
    let pan_map = build_axis_linear_map(
        status.pan_range.as_ref(),
        status.pan_position,
        DEFAULT_PAN_SPAN_UNITS,
        DEFAULT_PAN_MIN_DEG,
        DEFAULT_PAN_MAX_DEG,
    );

    let (tilt_deg_min, tilt_deg_max) = if status
        .tilt_range
        .as_ref()
        .is_some_and(|range| range.min >= 0)
    {
        (DEFAULT_UPWARD_TILT_MIN_DEG, DEFAULT_UPWARD_TILT_MAX_DEG)
    } else {
        (DEFAULT_TILT_MIN_DEG, DEFAULT_TILT_MAX_DEG)
    };

    let tilt_map = build_axis_linear_map(
        status.tilt_range.as_ref(),
        status.tilt_position,
        DEFAULT_TILT_SPAN_UNITS,
        tilt_deg_min,
        tilt_deg_max,
    );

    CalibrationParams {
        serial_number: device_info.serial_number.clone(),
        model: device_info.model.clone(),
        firmware: device_info.firmware.clone(),
        pan_offset: pan_map.offset,
        pan_scale: pan_map.scale,
        pan_deadband: pan_map.scale.abs().max(0.01),
        tilt_offset: tilt_map.offset,
        tilt_scale: tilt_map.scale,
        tilt_deadband: tilt_map.scale.abs().max(0.01),
        pan_model: AxisModelParams {
            alpha: DEFAULT_MODEL_ALPHA,
            beta: DEFAULT_MODEL_BETA,
        },
        tilt_model: AxisModelParams {
            alpha: DEFAULT_MODEL_ALPHA,
            beta: DEFAULT_MODEL_BETA,
        },
        created_at: now_epoch_millis().to_string(),
    }
}

#[derive(Debug, Clone, Copy)]
struct AxisLinearMap {
    offset: f64,
    scale: f64,
}

fn build_axis_linear_map(
    range: Option<&NumericRange>,
    current_position: Option<i64>,
    default_span_units: i64,
    deg_min: f64,
    deg_max: f64,
) -> AxisLinearMap {
    let (mut pos_min, mut pos_max) = match range {
        Some(range) if range.max > range.min => (range.min, range.max),
        Some(range) => (range.min, range.min + 1),
        None => {
            let center = current_position.unwrap_or(0);
            let half_span = (default_span_units / 2).max(1);
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

    let scale = (deg_max - deg_min) / (pos_max - pos_min) as f64;
    let offset = deg_min - pos_min as f64 * scale;

    AxisLinearMap { offset, scale }
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
