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
const CALIBRATION_RESTORE_CRUISE_SPEED: u8 = 6;
const CALIBRATION_RESTORE_PULSE_MS: u64 = 220;
const CALIBRATION_PULSE_SPEED_PAN: u8 = 6;
const CALIBRATION_PULSE_SPEED_TILT: u8 = 4;
const CALIBRATION_PULSE_MS_PAN: u64 = 220;
const CALIBRATION_PULSE_MS_TILT: u64 = 170;
const CALIBRATION_SETTLE_MS: u64 = 80;
const CALIBRATION_MIN_MOVE_DELTA_PAN: i64 = 8;
const CALIBRATION_MIN_MOVE_DELTA_TILT: i64 = 4;
const CALIBRATION_STALL_DELTA_PAN: i64 = 4;
const CALIBRATION_STALL_DELTA_TILT: i64 = 2;
const CALIBRATION_STALL_STEPS: usize = 3;
const CALIBRATION_SWEEP_MAX_STEPS: usize = 64;
const CALIBRATION_RESTORE_MAX_STEPS: usize = 36;
const CALIBRATION_RESTORE_TOLERANCE: i64 = 10;
const CALIBRATION_MODEL_SAMPLE_COUNT_PAN: usize = 100;
const CALIBRATION_MODEL_SAMPLE_COUNT_TILT: usize = 140;
const CALIBRATION_MODEL_MIN_SAMPLES_PAN: usize = 50;
const CALIBRATION_MODEL_MIN_SAMPLES_TILT: usize = 70;
const CALIBRATION_SWEEP_RETRY_PASSES_MAX: usize = 8;
const CALIBRATION_MODEL_TRIM_RATIO: f64 = 0.1;
const CALIBRATION_MODEL_RESIDUAL_BLEND_START_RATIO: f64 = 0.0025;
const CALIBRATION_MODEL_RESIDUAL_BLEND_END_RATIO: f64 = 0.0075;
const CALIBRATION_MODEL_RESIDUAL_BLEND_START_MULTIPLIER: f64 = 1.0;
const CALIBRATION_MODEL_RESIDUAL_BLEND_END_MULTIPLIER: f64 = 2.0;
const CALIBRATION_MODEL_RESIDUAL_BLEND_MIN_COUNT: f64 = 5.0;
const CALIBRATION_QUALITY_P95_MAX_RATIO_PAN: f64 = 0.02;
const CALIBRATION_QUALITY_P95_MAX_RATIO_TILT: f64 = 0.03;
const CALIBRATION_QUALITY_P95_FLOOR_PAN: i64 = 12;
const CALIBRATION_QUALITY_P95_FLOOR_TILT: i64 = 8;
const CALIBRATION_QUALITY_P95_CEILING_PAN: i64 = 220;
const CALIBRATION_QUALITY_P95_CEILING_TILT: i64 = 120;
const DEFAULT_DEADBAND_COUNT: i64 = 6;
const CALIBRATION_DEADBAND_PROBE_SPEED: u8 = 2;
const CALIBRATION_DEADBAND_PROBE_MS: u64 = 80;
const CALIBRATION_DEADBAND_PRELOAD_MS: u64 = 24;
const CALIBRATION_DEADBAND_PROBE_STEPS_MS: [u64; 9] = [
    8,
    12,
    16,
    24,
    32,
    48,
    64,
    CALIBRATION_DEADBAND_PROBE_MS,
    110,
];
const CALIBRATION_DEADBAND_PROBE_COUNT: usize = 7;
const CALIBRATION_DEADBAND_TRIM_RATIO: f64 = 0.2;
const CALIBRATION_DEADBAND_SPAN_CLIP_RATIO: f64 = 0.05;
const CALIBRATION_DEADBAND_SPAN_CLIP_MIN: i64 = DEFAULT_DEADBAND_COUNT;
const CALIBRATION_DEADBAND_SPAN_CLIP_MAX: i64 = 240;

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
        && can_reuse_saved_calibration(&status, channel, &saved_params)
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
            Ok((calibration, report)) => (calibration, report, CALIBRATION_SOURCE_MEASURED),
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

fn can_reuse_saved_calibration(
    status: &PtzStatus,
    requested_channel: u8,
    saved_params: &StoredCalibration,
) -> bool {
    status.calibrated() == Some(true) && saved_params.channel == requested_channel
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectionalDeadband {
    increase_count: i64,
    decrease_count: i64,
}

impl DirectionalDeadband {
    fn uniform(count: i64) -> Self {
        let clamped = count.max(1);
        Self {
            increase_count: clamped,
            decrease_count: clamped,
        }
    }

    fn compatibility_count(self) -> i64 {
        self.increase_count.max(self.decrease_count)
    }
}

fn try_build_measured_calibration(
    client: &Client,
    channel: u8,
    device_info: &DeviceInfo,
    status: &PtzStatus,
) -> AppResult<(CalibrationParams, CalibrationReport)> {
    let home_pan = axis_position(status, AxisKind::Pan)?;
    let home_tilt = axis_position(status, AxisKind::Tilt)?;
    let mut pan_motion = None;
    let mut tilt_motion = None;

    let measured = (|| {
        let detected_pan_motion = detect_axis_motion(client, channel, AxisKind::Pan)?;
        pan_motion = Some(detected_pan_motion);
        let detected_tilt_motion = detect_axis_motion(client, channel, AxisKind::Tilt)?;
        tilt_motion = Some(detected_tilt_motion);

        let (pan_min, pan_max, pan_sweep_deltas) =
            sweep_axis_bounds(client, channel, AxisKind::Pan, detected_pan_motion)?;
        let (tilt_min, tilt_max, tilt_sweep_deltas) =
            sweep_axis_bounds(client, channel, AxisKind::Tilt, detected_tilt_motion)?;

        restore_axis_to_home(
            client,
            channel,
            AxisKind::Pan,
            home_pan,
            detected_pan_motion,
        )?;
        restore_axis_to_home(
            client,
            channel,
            AxisKind::Tilt,
            home_tilt,
            detected_tilt_motion,
        )?;
        let pan_span = (pan_max - pan_min).unsigned_abs() as f64;
        let tilt_span = (tilt_max - tilt_min).unsigned_abs() as f64;
        let pan_estimate =
            estimate_model_from_sweep_with_quality(AxisKind::Pan, pan_span, &pan_sweep_deltas);
        let tilt_estimate =
            estimate_model_from_sweep_with_quality(AxisKind::Tilt, tilt_span, &tilt_sweep_deltas);
        validate_measured_calibration_quality(
            pan_span,
            tilt_span,
            pan_estimate.residual_p95_count,
            tilt_estimate.residual_p95_count,
        )?;
        let pan_deadband = estimate_directional_deadband_count(
            client,
            channel,
            AxisKind::Pan,
            detected_pan_motion,
            pan_span,
        )
        .unwrap_or_else(|_| DirectionalDeadband::uniform(DEFAULT_DEADBAND_COUNT));
        let tilt_deadband = estimate_directional_deadband_count(
            client,
            channel,
            AxisKind::Tilt,
            detected_tilt_motion,
            tilt_span,
        )
        .unwrap_or_else(|_| DirectionalDeadband::uniform(DEFAULT_DEADBAND_COUNT));
        let pan_deadband_count = pan_deadband.compatibility_count();
        let tilt_deadband_count = tilt_deadband.compatibility_count();

        Ok((
            CalibrationParams {
                serial_number: device_info.serial_number.clone(),
                model: device_info.model.clone(),
                firmware: device_info.firmware.clone(),
                pan_min_count: pan_min,
                pan_max_count: pan_max,
                pan_deadband_count,
                pan_deadband_increase_count: Some(pan_deadband.increase_count),
                pan_deadband_decrease_count: Some(pan_deadband.decrease_count),
                tilt_min_count: tilt_min,
                tilt_max_count: tilt_max,
                tilt_deadband_count,
                tilt_deadband_increase_count: Some(tilt_deadband.increase_count),
                tilt_deadband_decrease_count: Some(tilt_deadband.decrease_count),
                pan_model: pan_estimate.model,
                tilt_model: tilt_estimate.model,
                created_at: now_epoch_millis().to_string(),
            },
            CalibrationReport {
                samples: pan_estimate.sample_count.max(tilt_estimate.sample_count),
                pan_error_p95_count: pan_estimate.residual_p95_count,
                tilt_error_p95_count: tilt_estimate.residual_p95_count,
                notes: format!(
                    "created_from_measured_sweep; pan_samples={}; tilt_samples={}; pan_blend={:.2}; tilt_blend={:.2}",
                    pan_estimate.sample_count,
                    tilt_estimate.sample_count,
                    pan_estimate.fallback_blend_ratio,
                    tilt_estimate.fallback_blend_ratio
                ),
            },
        ))
    })();

    attempt_home_restore_on_failure(
        &measured,
        home_pan,
        home_tilt,
        pan_motion,
        tilt_motion,
        |axis, home_position, motion| {
            let _ = restore_axis_to_home(client, channel, axis, home_position, motion);
        },
    );

    let _ = ptz_transport::stop_ptz(client, channel);
    measured
}

fn attempt_home_restore_on_failure<T, F>(
    measured_result: &AppResult<T>,
    home_pan: i64,
    home_tilt: i64,
    pan_motion: Option<AxisMotion>,
    tilt_motion: Option<AxisMotion>,
    mut restore_axis: F,
) where
    F: FnMut(AxisKind, i64, AxisMotion),
{
    if measured_result.is_ok() {
        return;
    }

    if let Some(pan_motion) = pan_motion {
        restore_axis(AxisKind::Pan, home_pan, pan_motion);
    }

    if let Some(tilt_motion) = tilt_motion {
        restore_axis(AxisKind::Tilt, home_tilt, tilt_motion);
    }
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
    pulse(client, channel, axis, primary)?;
    let after_primary = read_axis_position(client, channel, axis)?;
    let primary_delta = after_primary - before_primary;
    pulse(client, channel, axis, secondary)?;

    if primary_delta.abs() >= calibration_min_move_delta(axis) {
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
    pulse(client, channel, axis, secondary)?;
    let after_secondary = read_axis_position(client, channel, axis)?;
    let secondary_delta = after_secondary - before_secondary;
    pulse(client, channel, axis, primary)?;

    if secondary_delta.abs() < calibration_min_move_delta(axis) {
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
    let sample_cap = axis_model_sample_cap(axis);
    let min_samples = axis_model_min_samples(axis).min(sample_cap);
    let mut sweep_deltas = Vec::with_capacity(sample_cap);
    let mut min = sweep_axis_limit(
        client,
        channel,
        axis,
        motion.decrease,
        true,
        &mut sweep_deltas,
        sample_cap,
    )?;
    let mut max = sweep_axis_limit(
        client,
        channel,
        axis,
        motion.increase,
        false,
        &mut sweep_deltas,
        sample_cap,
    )?;
    if sweep_deltas.len() < min_samples {
        for _ in 0..CALIBRATION_SWEEP_RETRY_PASSES_MAX {
            let pass_min = sweep_axis_limit(
                client,
                channel,
                axis,
                motion.decrease,
                true,
                &mut sweep_deltas,
                sample_cap,
            )?;
            min = min.min(pass_min);
            let pass_max = sweep_axis_limit(
                client,
                channel,
                axis,
                motion.increase,
                false,
                &mut sweep_deltas,
                sample_cap,
            )?;
            max = max.max(pass_max);
            if sweep_deltas.len() >= min_samples {
                break;
            }
        }
    }
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
    sample_cap: usize,
) -> AppResult<i64> {
    let mut best = read_axis_position(client, channel, axis)?;
    let mut stall_steps = 0usize;

    for _ in 0..CALIBRATION_SWEEP_MAX_STEPS {
        let before = read_axis_position(client, channel, axis)?;
        pulse(client, channel, axis, direction)?;
        let after = read_axis_position(client, channel, axis)?;
        let moved_count = (after - before).unsigned_abs() as f64;
        if sweep_deltas.len() < sample_cap && moved_count.is_finite() && moved_count > 0.0 {
            sweep_deltas.push(moved_count);
        }

        best = if toward_min {
            best.min(after)
        } else {
            best.max(after)
        };

        if (after - before).abs() <= calibration_stall_delta(axis) {
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
            CALIBRATION_RESTORE_CRUISE_SPEED
        };

        ptz_transport::move_ptz(
            client,
            channel,
            direction,
            speed,
            Some(CALIBRATION_RESTORE_PULSE_MS),
        )?;
        thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    }

    Ok(())
}

fn estimate_directional_deadband_count(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    motion: AxisMotion,
    span: f64,
) -> AppResult<DirectionalDeadband> {
    let mut increase_samples = Vec::with_capacity(CALIBRATION_DEADBAND_PROBE_COUNT);
    let mut decrease_samples = Vec::with_capacity(CALIBRATION_DEADBAND_PROBE_COUNT);

    for _ in 0..CALIBRATION_DEADBAND_PROBE_COUNT {
        let sample = measure_directional_deadband_probe(client, channel, axis, motion)?;
        increase_samples.push(sample.increase_count);
        decrease_samples.push(sample.decrease_count);
    }

    Ok(DirectionalDeadband {
        increase_count: estimate_deadband_from_samples(&increase_samples, span),
        decrease_count: estimate_deadband_from_samples(&decrease_samples, span),
    })
}

fn measure_directional_deadband_probe(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    motion: AxisMotion,
) -> AppResult<DirectionalDeadband> {
    let increase_count = measure_directional_deadband_count(
        client,
        channel,
        axis,
        motion.increase,
        motion.decrease,
    )?;
    let decrease_count = measure_directional_deadband_count(
        client,
        channel,
        axis,
        motion.decrease,
        motion.increase,
    )?;
    Ok(DirectionalDeadband {
        increase_count,
        decrease_count,
    })
}

fn measure_directional_deadband_count(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    probe_direction: crate::core::model::PtzDirection,
    preload_direction: crate::core::model::PtzDirection,
) -> AppResult<i64> {
    ptz_transport::move_ptz(
        client,
        channel,
        preload_direction,
        CALIBRATION_DEADBAND_PROBE_SPEED,
        Some(CALIBRATION_DEADBAND_PRELOAD_MS),
    )?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));

    let min_move = calibration_min_move_delta(axis).max(1);

    for pulse_ms in CALIBRATION_DEADBAND_PROBE_STEPS_MS {
        let before = read_axis_position(client, channel, axis)?;
        ptz_transport::move_ptz(
            client,
            channel,
            probe_direction,
            CALIBRATION_DEADBAND_PROBE_SPEED,
            Some(pulse_ms),
        )?;
        thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
        let after = read_axis_position(client, channel, axis)?;
        let delta = (after - before).abs().max(1);
        if delta >= min_move {
            return Ok(delta);
        }
    }

    Ok(min_move)
}

fn estimate_deadband_from_samples(samples: &[i64], span: f64) -> i64 {
    let robust = robust_deadband_from_samples(samples).unwrap_or(DEFAULT_DEADBAND_COUNT);
    clip_deadband_estimate(robust, span)
}

fn robust_deadband_from_samples(samples: &[i64]) -> Option<i64> {
    let mut sorted = samples
        .iter()
        .copied()
        .filter(|sample| *sample > 0)
        .collect::<Vec<_>>();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable();

    let median = median_of_sorted_i64(&sorted);
    let trimmed_mean = trimmed_mean_of_sorted_i64(&sorted, CALIBRATION_DEADBAND_TRIM_RATIO);
    let blended = ((median + trimmed_mean) * 0.5).round() as i64;
    Some(blended.max(1))
}

fn median_of_sorted_i64(sorted: &[i64]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }

    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid] as f64
    } else {
        (sorted[mid - 1] as f64 + sorted[mid] as f64) * 0.5
    }
}

fn trimmed_mean_of_sorted_i64(sorted: &[i64], trim_ratio: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }

    let max_trim = sorted.len().saturating_sub(1) / 2;
    let requested_trim = (sorted.len() as f64 * trim_ratio).floor() as usize;
    let trim = requested_trim.min(max_trim);
    let trimmed = &sorted[trim..(sorted.len() - trim)];
    let sum = trimmed.iter().sum::<i64>();
    sum as f64 / trimmed.len() as f64
}

fn clip_deadband_estimate(raw_deadband: i64, span: f64) -> i64 {
    let upper_bound = deadband_upper_bound_for_span(span);
    raw_deadband.abs().max(1).min(upper_bound)
}

fn deadband_upper_bound_for_span(span: f64) -> i64 {
    let span_based_cap = (span.abs() * CALIBRATION_DEADBAND_SPAN_CLIP_RATIO).round() as i64;
    span_based_cap
        .clamp(
            CALIBRATION_DEADBAND_SPAN_CLIP_MIN,
            CALIBRATION_DEADBAND_SPAN_CLIP_MAX,
        )
        .max(1)
}

fn pulse(
    client: &Client,
    channel: u8,
    axis: AxisKind,
    direction: crate::core::model::PtzDirection,
) -> AppResult<()> {
    ptz_transport::move_ptz(
        client,
        channel,
        direction,
        calibration_pulse_speed(axis),
        Some(calibration_pulse_ms(axis)),
    )?;
    thread::sleep(Duration::from_millis(CALIBRATION_SETTLE_MS));
    Ok(())
}

fn calibration_pulse_speed(axis: AxisKind) -> u8 {
    match axis {
        AxisKind::Pan => CALIBRATION_PULSE_SPEED_PAN,
        AxisKind::Tilt => CALIBRATION_PULSE_SPEED_TILT,
    }
}

fn calibration_pulse_ms(axis: AxisKind) -> u64 {
    match axis {
        AxisKind::Pan => CALIBRATION_PULSE_MS_PAN,
        AxisKind::Tilt => CALIBRATION_PULSE_MS_TILT,
    }
}

fn calibration_min_move_delta(axis: AxisKind) -> i64 {
    match axis {
        AxisKind::Pan => CALIBRATION_MIN_MOVE_DELTA_PAN,
        AxisKind::Tilt => CALIBRATION_MIN_MOVE_DELTA_TILT,
    }
}

fn calibration_stall_delta(axis: AxisKind) -> i64 {
    match axis {
        AxisKind::Pan => CALIBRATION_STALL_DELTA_PAN,
        AxisKind::Tilt => CALIBRATION_STALL_DELTA_TILT,
    }
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
        pan_deadband_increase_count: Some(DEFAULT_DEADBAND_COUNT),
        pan_deadband_decrease_count: Some(DEFAULT_DEADBAND_COUNT),
        tilt_min_count: tilt_range.min_count,
        tilt_max_count: tilt_range.max_count,
        tilt_deadband_count: DEFAULT_DEADBAND_COUNT,
        tilt_deadband_increase_count: Some(DEFAULT_DEADBAND_COUNT),
        tilt_deadband_decrease_count: Some(DEFAULT_DEADBAND_COUNT),
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

#[derive(Debug, Clone, Copy)]
struct AxisModelEstimate {
    model: AxisModelParams,
    residual_p95_count: i64,
    fallback_blend_ratio: f64,
    sample_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct AxisQualityThreshold {
    ratio: f64,
    floor_count: i64,
    ceiling_count: i64,
}

fn axis_quality_threshold(axis: AxisKind) -> AxisQualityThreshold {
    match axis {
        AxisKind::Pan => AxisQualityThreshold {
            ratio: CALIBRATION_QUALITY_P95_MAX_RATIO_PAN,
            floor_count: CALIBRATION_QUALITY_P95_FLOOR_PAN,
            ceiling_count: CALIBRATION_QUALITY_P95_CEILING_PAN,
        },
        AxisKind::Tilt => AxisQualityThreshold {
            ratio: CALIBRATION_QUALITY_P95_MAX_RATIO_TILT,
            floor_count: CALIBRATION_QUALITY_P95_FLOOR_TILT,
            ceiling_count: CALIBRATION_QUALITY_P95_CEILING_TILT,
        },
    }
}

fn axis_model_sample_cap(axis: AxisKind) -> usize {
    match axis {
        AxisKind::Pan => CALIBRATION_MODEL_SAMPLE_COUNT_PAN,
        AxisKind::Tilt => CALIBRATION_MODEL_SAMPLE_COUNT_TILT,
    }
}

fn axis_model_min_samples(axis: AxisKind) -> usize {
    match axis {
        AxisKind::Pan => CALIBRATION_MODEL_MIN_SAMPLES_PAN,
        AxisKind::Tilt => CALIBRATION_MODEL_MIN_SAMPLES_TILT,
    }
}

fn axis_quality_threshold_count(axis: AxisKind, span: f64) -> i64 {
    let threshold = axis_quality_threshold(axis);
    let span_based = (span.abs() * threshold.ratio).round() as i64;
    span_based
        .clamp(threshold.floor_count, threshold.ceiling_count)
        .max(1)
}

fn validate_measured_calibration_quality(
    pan_span: f64,
    tilt_span: f64,
    pan_residual_p95_count: i64,
    tilt_residual_p95_count: i64,
) -> AppResult<()> {
    let pan_threshold = axis_quality_threshold(AxisKind::Pan);
    let tilt_threshold = axis_quality_threshold(AxisKind::Tilt);
    let pan_max_allowed = axis_quality_threshold_count(AxisKind::Pan, pan_span);
    let tilt_max_allowed = axis_quality_threshold_count(AxisKind::Tilt, tilt_span);

    if pan_residual_p95_count <= pan_max_allowed && tilt_residual_p95_count <= tilt_max_allowed {
        return Ok(());
    }

    Err(AppError::new(
        ErrorKind::UnexpectedResponse,
        format!(
            "measured calibration rejected by quality gate: pan_p95={pan_residual_p95_count} (max={pan_max_allowed}, ratio={:.4}, floor={}, ceiling={}, span={:.0}); tilt_p95={tilt_residual_p95_count} (max={tilt_max_allowed}, ratio={:.4}, floor={}, ceiling={}, span={:.0})",
            pan_threshold.ratio,
            pan_threshold.floor_count,
            pan_threshold.ceiling_count,
            pan_span,
            tilt_threshold.ratio,
            tilt_threshold.floor_count,
            tilt_threshold.ceiling_count,
            tilt_span,
        ),
    ))
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

#[cfg(test)]
fn estimate_model_from_sweep(span: f64, sweep_deltas: &[f64]) -> AxisModelParams {
    estimate_model_from_sweep_with_quality(AxisKind::Pan, span, sweep_deltas).model
}

fn estimate_model_from_sweep_with_quality(
    axis: AxisKind,
    span: f64,
    sweep_deltas: &[f64],
) -> AxisModelEstimate {
    let fallback = fallback_model_for_span(span);
    let sample_cap = axis_model_sample_cap(axis);
    let mut samples = sweep_deltas
        .iter()
        .copied()
        .filter(|delta| delta.is_finite() && *delta > 0.0)
        .collect::<Vec<_>>();
    if samples.len() > sample_cap {
        samples = evenly_spaced_samples(&samples, sample_cap);
    }
    if samples.len() < 2 {
        return AxisModelEstimate {
            model: fallback,
            residual_p95_count: 0,
            fallback_blend_ratio: 1.0,
            sample_count: samples.len(),
        };
    }

    let stabilized = winsorize_samples(&samples, CALIBRATION_MODEL_TRIM_RATIO);
    let estimated = estimate_model_from_samples(axis, &stabilized, fallback);
    let estimated_p95 = model_residual_p95_count(axis, &samples, estimated);
    let fallback_blend_ratio = residual_fallback_blend_ratio(estimated_p95 as f64, span, &samples);
    let blended = blend_axis_models(estimated, fallback, fallback_blend_ratio);
    let blended_p95 = model_residual_p95_count(axis, &samples, blended);

    AxisModelEstimate {
        model: blended,
        residual_p95_count: blended_p95,
        fallback_blend_ratio,
        sample_count: samples.len(),
    }
}

fn estimate_model_from_samples(
    axis: AxisKind,
    samples: &[f64],
    fallback: AxisModelParams,
) -> AxisModelParams {
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
    let velocity = mean_delta / calibration_effective_ts_sec(axis);
    let beta = (velocity * (1.0 - alpha) / calibration_control_u(axis))
        .clamp(MODEL_BETA_MIN, MODEL_BETA_MAX);
    if !alpha.is_finite() || !beta.is_finite() {
        return fallback;
    }

    AxisModelParams { alpha, beta }
}

fn winsorize_samples(samples: &[f64], trim_ratio: f64) -> Vec<f64> {
    if samples.len() < 4 {
        return samples.to_vec();
    }

    let trim = trim_ratio.clamp(0.0, 0.49);
    let mut sorted = samples.to_vec();
    sorted.sort_by(|lhs, rhs| lhs.total_cmp(rhs));
    let lower = quantile_of_sorted_f64(&sorted, trim);
    let upper = quantile_of_sorted_f64(&sorted, 1.0 - trim);
    samples
        .iter()
        .map(|sample| sample.clamp(lower, upper))
        .collect()
}

fn quantile_of_sorted_f64(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let q = quantile.clamp(0.0, 1.0);
    let position = q * (sorted.len() - 1) as f64;
    let lower_index = position.floor() as usize;
    let upper_index = position.ceil() as usize;
    if lower_index == upper_index {
        return sorted[lower_index];
    }

    let fraction = position - lower_index as f64;
    sorted[lower_index] + (sorted[upper_index] - sorted[lower_index]) * fraction
}

fn model_residual_p95_count(axis: AxisKind, samples: &[f64], model: AxisModelParams) -> i64 {
    if samples.len() < 2 {
        return 0;
    }

    let input_gain = model.beta * calibration_control_u(axis) * calibration_effective_ts_sec(axis);
    let mut residuals = Vec::with_capacity(samples.len().saturating_sub(1));
    for window in samples.windows(2) {
        let predicted = model.alpha * window[0] + input_gain;
        let residual = (window[1] - predicted).abs();
        if residual.is_finite() {
            residuals.push(residual);
        }
    }
    percentile_count_from_f64(&residuals, 0.95)
}

fn percentile_count_from_f64(samples: &[f64], quantile: f64) -> i64 {
    if samples.is_empty() {
        return 0;
    }

    let mut sorted = samples.to_vec();
    sorted.sort_by(|lhs, rhs| lhs.total_cmp(rhs));
    let q = quantile.clamp(0.0, 1.0);
    let index = ((sorted.len().saturating_sub(1)) as f64 * q).ceil() as usize;
    let bounded_index = index.min(sorted.len().saturating_sub(1));
    sorted[bounded_index].round().max(0.0) as i64
}

fn residual_fallback_blend_ratio(residual_p95: f64, span: f64, samples: &[f64]) -> f64 {
    if !residual_p95.is_finite() {
        return 1.0;
    }

    let mean_delta = if samples.is_empty() {
        0.0
    } else {
        samples.iter().sum::<f64>() / samples.len() as f64
    };
    let start = CALIBRATION_MODEL_RESIDUAL_BLEND_MIN_COUNT.max(
        (span.abs() * CALIBRATION_MODEL_RESIDUAL_BLEND_START_RATIO)
            .max(mean_delta * CALIBRATION_MODEL_RESIDUAL_BLEND_START_MULTIPLIER),
    );
    let end = (start + 1.0).max(
        (span.abs() * CALIBRATION_MODEL_RESIDUAL_BLEND_END_RATIO)
            .max(mean_delta * CALIBRATION_MODEL_RESIDUAL_BLEND_END_MULTIPLIER),
    );

    ((residual_p95 - start) / (end - start)).clamp(0.0, 1.0)
}

fn blend_axis_models(
    estimated: AxisModelParams,
    fallback: AxisModelParams,
    blend_ratio: f64,
) -> AxisModelParams {
    let blend = blend_ratio.clamp(0.0, 1.0);
    let keep = 1.0 - blend;
    AxisModelParams {
        alpha: (estimated.alpha * keep + fallback.alpha * blend)
            .clamp(MODEL_ALPHA_MIN, MODEL_ALPHA_MAX),
        beta: (estimated.beta * keep + fallback.beta * blend).clamp(MODEL_BETA_MIN, MODEL_BETA_MAX),
    }
}

fn evenly_spaced_samples(samples: &[f64], target_count: usize) -> Vec<f64> {
    if samples.len() <= target_count {
        return samples.to_vec();
    }
    if target_count == 0 {
        return Vec::new();
    }
    if target_count == 1 {
        return vec![samples[samples.len() / 2]];
    }

    let last_index = samples.len() - 1;
    let mut selected = Vec::with_capacity(target_count);
    for i in 0..target_count {
        let index = i * last_index / (target_count - 1);
        selected.push(samples[index]);
    }
    selected
}

fn fallback_model_for_span(span: f64) -> AxisModelParams {
    AxisModelParams {
        alpha: DEFAULT_MODEL_ALPHA,
        beta: (span.abs().max(1.0) * DEFAULT_MODEL_BETA_RATIO)
            .clamp(MODEL_BETA_MIN, MODEL_BETA_MAX),
    }
}

fn calibration_control_u(axis: AxisKind) -> f64 {
    let speed_factor = (calibration_pulse_speed(axis) as f64 / 64.0).clamp(0.0, 1.0);
    let pulse_factor = (calibration_pulse_ms(axis) as f64 / 120.0).clamp(0.5, 1.5);
    (speed_factor * pulse_factor).clamp(1e-3, 1.0)
}

fn calibration_effective_ts_sec(axis: AxisKind) -> f64 {
    ((calibration_pulse_ms(axis) + CALIBRATION_SETTLE_MS) as f64 / 1_000.0).clamp(0.05, 0.5)
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
mod tests;
