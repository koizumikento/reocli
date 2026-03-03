use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::usecases::ptz_calibrate_auto::{StoredCalibration, load_saved_params_for_device};
use crate::app::usecases::ptz_controller::{AxisEkf, AxisEkfConfig, AxisEkfSnapshot};
use crate::app::usecases::ptz_deadband::scale_directional_deadband;
use crate::app::usecases::ptz_get_absolute_raw::{PtzRawPosition, map_status_to_raw_position};
use crate::app::usecases::ptz_pulse_lut::{AxisDirection, AxisPulseLut};
use crate::app::usecases::ptz_settle_gate::{
    PositionSettlingTracker, completion_gate_allows_success,
};
use crate::app::usecases::ptz_transport;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{AxisModelParams, NumericRange, PtzDirection};
use crate::interfaces::runtime;
use crate::reolink::client::Client;
use crate::reolink::{device, ptz};

const EKF_TS_SEC: f64 = 0.08;
const EKF_STATE_SCHEMA_VERSION: u32 = 1;
const MIN_ADAPTIVE_UPDATES: usize = 2;
const REQUIRED_STABLE_STEPS: usize = 2;
const SETTLE_STEP_MS: u64 = 100;
const TIMEOUT_RETRY_BUDGET_MIN_MS: u64 = 18_000;
const TIMEOUT_RETRY_BUDGET_MAX_MS: u64 = 36_000;
const MIN_CONTROL_PULSE_MS: u64 = 0;
const MAX_CONTROL_PULSE_MS: u64 = 220;
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
const FINE_RELATIVE_STEP_GAIN: f64 = 0.55;
const FINE_RELATIVE_STEP_MIN_COUNT: f64 = 4.0;
const FINE_RELATIVE_STEP_MAX_COUNT: f64 = 96.0;
const FINE_FEEDFORWARD_GAIN: f64 = 0.28;
const FINE_FEEDFORWARD_MAX_COUNT: f64 = 72.0;
const BACKEND_COMPLETION_MIN_AGE_MS: u64 = 120;
const BACKEND_POSITION_STABLE_REQUIRED_STEPS: usize = 2;
const BACKEND_POSITION_STABLE_TOLERANCE_RATIO: f64 = 0.35;
const BACKEND_POSITION_STABLE_MIN_COUNT: f64 = 2.0;
const BACKEND_POSITION_STABLE_MAX_COUNT: f64 = 24.0;
const REVERSAL_GUARD_MULTIPLIER: f64 = 4.0;
const REVERSAL_GUARD_MIN_COUNT: f64 = 40.0;
const REVERSAL_GUARD_MOMENTUM_MIN_SCALE: f64 = 0.6;
const REVERSAL_GUARD_NEAR_TARGET_RATIO: f64 = 1.8;
const REVERSAL_GUARD_NEAR_TARGET_MIN_COUNT: f64 = 26.0;
const REVERSAL_GUARD_NEAR_TARGET_MIN_SCALE: f64 = 0.24;
const DUAL_AXIS_DOMINANCE_RATIO: f64 = 1.2;
const TIE_BREAK_CLOSE_ERROR_COUNT: f64 = 320.0;
const TILT_EDGE_CONTROL_MARGIN_COUNT: f64 = 120.0;
const TILT_EDGE_CONTROL_MAX_PULSE_MS: u64 = 20;
const TILT_EDGE_CONTROL_SPEED_CAP: u8 = 1;
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
const OSCILLATION_TOLERANCE_RELAX_RATIO: f64 = 0.22;
const OSCILLATION_TOLERANCE_RELAX_MAX_COUNT: f64 = 48.0;
const CALIBRATION_DEADBAND_HINT_MAX_COUNT: f64 = 200.0;
const CALIBRATION_GUARD_DEADBAND_RATIO: f64 = 0.45;

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
const OSCILLATION_STABLE_STEP_REDUCTION_REVERSAL_SUM: usize = 6;
const SECONDARY_AXIS_INTERLEAVE_MIN_NORM: f64 = 0.02;
const SECONDARY_AXIS_INTERLEAVE_INTERVAL_MIN: usize = 1;
const SECONDARY_AXIS_INTERLEAVE_INTERVAL_MAX: usize = 5;

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

pub fn execute(
    client: &Client,
    channel: u8,
    target_pan_count: i64,
    target_tilt_count: i64,
    tolerance_count: i64,
    timeout_ms: u64,
) -> AppResult<PtzRawPosition> {
    validate_inputs(tolerance_count, timeout_ms)?;

    let (state_key, state_path) = ekf_state_identity(client, channel);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut operation_result = run_closed_loop(
        client,
        channel,
        target_pan_count,
        target_tilt_count,
        tolerance_count,
        timeout_ms,
        deadline,
        &state_key,
        &state_path,
    );

    if let Err(initial_error) = operation_result {
        if should_retry_after_timeout(&initial_error) {
            let retry_timeout_ms = timeout_retry_budget_ms(timeout_ms);
            let retry_deadline = Instant::now() + Duration::from_millis(retry_timeout_ms);
            operation_result = match run_closed_loop(
                client,
                channel,
                target_pan_count,
                target_tilt_count,
                tolerance_count,
                retry_timeout_ms,
                retry_deadline,
                &state_key,
                &state_path,
            ) {
                Ok(result) => Ok(result),
                Err(retry_error) => {
                    let initial_modes =
                        parse_failure_mode_counters(&initial_error.message).unwrap_or_default();
                    let retry_modes =
                        parse_failure_mode_counters(&retry_error.message).unwrap_or_default();
                    let max_modes = max_failure_mode_counters(initial_modes, retry_modes);
                    Err(AppError::new(
                        retry_error.kind,
                        format!(
                            "initial_timeout='{}'; retry_timeout='{}'; initial_failure_modes={}; retry_failure_modes={}; failure_modes_max={}",
                            initial_error.message,
                            retry_error.message,
                            format_failure_mode_counters(initial_modes),
                            format_failure_mode_counters(retry_modes),
                            format_failure_mode_counters(max_modes),
                        ),
                    ))
                }
            };
        } else {
            operation_result = Err(initial_error);
        }
    }

    finalize_with_best_effort_stop(client, channel, operation_result)
}

fn should_retry_after_timeout(error: &AppError) -> bool {
    error.kind == ErrorKind::UnexpectedResponse
        && error.message.contains("set_absolute_raw timeout")
}

fn timeout_retry_budget_ms(timeout_ms: u64) -> u64 {
    let scaled = timeout_ms.saturating_mul(3);
    scaled.clamp(TIMEOUT_RETRY_BUDGET_MIN_MS, TIMEOUT_RETRY_BUDGET_MAX_MS)
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
    deadline: Instant,
    state_key: &str,
    state_path: &Path,
) -> AppResult<PtzRawPosition> {
    let status_with_ranges = ptz::get_ptz_status(client, channel).ok();
    let saved_calibration = load_saved_calibration_for_channel(client, channel);
    let initial_status = ptz::get_ptz_cur_pos(client, channel)?;
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
    let pan_success_tolerance = axis_one_percent_threshold(pan_nominal_span);
    let tilt_success_tolerance = axis_one_percent_threshold(tilt_nominal_span);
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
    let mut stale_status_streak = 0usize;
    let mut pan_reversals = 0usize;
    let mut tilt_reversals = 0usize;
    let mut prev_pan_error_measured: Option<f64> = None;
    let mut prev_tilt_error_measured: Option<f64> = None;
    let started = Instant::now();
    let onvif_options = ptz_transport::get_onvif_configuration_options(client, channel)
        .ok()
        .flatten();
    let deadband_hints = load_axis_deadband_hints(saved_calibration.as_ref());
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
    }

    loop {
        let status = ptz::get_ptz_cur_pos(client, channel)?;
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
        update_reversal_counter(
            &mut pan_reversals,
            &mut prev_pan_error_measured,
            pan_error_measured,
            tolerance_count as f64,
        );
        update_reversal_counter(
            &mut tilt_reversals,
            &mut prev_tilt_error_measured,
            tilt_error_measured,
            tolerance_count as f64,
        );
        let pan_tolerance = adaptive_axis_tolerance(
            tolerance_count as f64,
            pan_reversals,
            scale_directional_deadband(
                deadband_hints.pan_count,
                pan_measure,
                pan_min_count,
                pan_max_count,
            ),
        );
        let tilt_tolerance = adaptive_axis_tolerance(
            tolerance_count as f64,
            tilt_reversals,
            scale_directional_deadband(
                deadband_hints.tilt_count,
                tilt_measure,
                tilt_min_count,
                tilt_max_count,
            ),
        );

        let pan_error_control =
            select_control_error(pan_error_estimated, pan_error_measured, tolerance_count);
        let tilt_error_control =
            select_control_error(tilt_error_estimated, tilt_error_measured, tolerance_count);
        let pan_observed_delta = previous_pan_measure.map(|previous| pan_measure - previous);
        let tilt_observed_delta = previous_tilt_measure.map(|previous| tilt_measure - previous);
        previous_pan_measure = Some(pan_measure);
        previous_tilt_measure = Some(tilt_measure);
        let pan_predicted_delta = pan_gain_tracker.predicted_delta(last_pan_u);
        let tilt_predicted_delta = tilt_gain_tracker.predicted_delta(last_tilt_u);
        if model_mismatch_detected(pan_predicted_delta, pan_observed_delta)
            || model_mismatch_detected(tilt_predicted_delta, tilt_observed_delta)
        {
            failure_modes.model_mismatch_hits = failure_modes.model_mismatch_hits.saturating_add(1);
        }
        pan_gain_tracker.observe(last_pan_u, pan_observed_delta, pan_span);
        tilt_gain_tracker.observe(last_tilt_u, tilt_observed_delta, tilt_span);
        if stale_status_detected(
            last_pan_u,
            last_tilt_u,
            pan_observed_delta,
            tilt_observed_delta,
            &mut stale_status_streak,
        ) {
            failure_modes.stale_status_hits = failure_modes.stale_status_hits.saturating_add(1);
        }
        apply_pending_pulse_observation(
            &mut pending_pulse_observation,
            pan_observed_delta,
            tilt_observed_delta,
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
                <= FINE_PHASE_ENTRY_ERROR_COUNT;
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
            tolerance_count as f64,
            pan_guard_deadband,
        );
        let guarded_tilt_error = apply_reversal_guard(
            tilt_error_control,
            last_tilt_u,
            tolerance_count as f64,
            tilt_guard_deadband,
        );
        let pan_command_error = if fine_phase_candidate {
            apply_fine_phase_feedforward(
                guarded_pan_error,
                pan_gain_tracker.beta_for_u(last_pan_u),
                last_pan_u,
                pan_observed_delta,
                tolerance_count as f64,
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
                tolerance_count as f64,
            )
        } else {
            guarded_tilt_error
        };
        let command_error_norm =
            normalized_vector_error(pan_command_error, tilt_command_error, pan_span, tilt_span);

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

        let within_tolerance = pan_error_measured.abs() <= pan_success_tolerance
            && tilt_error_measured.abs() <= tilt_success_tolerance;
        let required_stable_steps =
            required_stable_steps_for_oscillation(pan_reversals, tilt_reversals);
        let backend_motion_hint = ptz_transport::motion_status_hint(client, channel);
        let backend_completion_ready = if let Some(hint) = backend_motion_hint {
            completion_gate_allows_success(
                hint.moving,
                hint.move_age_ms,
                backend_completion_min_age_ms,
                position_settling.stable_steps(),
                BACKEND_POSITION_STABLE_REQUIRED_STEPS,
            )
        } else {
            position_settling.stable_steps() >= BACKEND_POSITION_STABLE_REQUIRED_STEPS
        };
        if within_tolerance && loop_updates >= min_updates && backend_completion_ready {
            stable_steps += 1;
        } else {
            stable_steps = 0;
        }

        if within_tolerance && stable_steps >= required_stable_steps {
            save_stored_ekf_state(
                state_path,
                state_key,
                channel,
                &pan_filter,
                &tilt_filter,
                0.0,
                0.0,
            )?;
            return Ok(current);
        }

        if Instant::now() >= deadline {
            let best = best_observed.unwrap_or(BestObservedState {
                pan_count: current.pan_count,
                tilt_count: current.tilt_count,
                pan_abs_error: pan_error_measured.abs().round() as i64,
                tilt_abs_error: tilt_error_measured.abs().round() as i64,
            });
            let best_within_success_tol =
                best_within_success_tolerance(best, pan_success_tolerance, tilt_success_tolerance);
            let latch_eligible = best_within_success_tol
                && success_latch_ready(
                    position_settling.stable_steps(),
                    measured_error_norm,
                    backend_motion_hint,
                );
            if (within_tolerance && backend_completion_ready) || latch_eligible {
                let success_position = if within_tolerance && backend_completion_ready {
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
                    0.0,
                    0.0,
                )?;
                return Ok(success_position);
            }
            let persist_error = save_stored_ekf_state(
                state_path,
                state_key,
                channel,
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
            let timeout_blocker = timeout_blocker_label(
                within_tolerance,
                backend_completion_ready,
                best_within_success_tol,
                latch_eligible,
            );
            let dominant_failure_mode = dominant_failure_mode_label(failure_modes);
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "set_absolute_raw timeout after {}ms on channel {channel}: target=({},{}) current=({},{}) measured_error=({:.1},{:.1}) measured_error_norm={:.5} estimated_error=({:.1},{:.1}) control_error=({:.1},{:.1}) command_error=({:.1},{:.1}) command_error_norm={:.5} tolerance={} control_tolerance=({:.1},{:.1}) success_tolerance=({:.1},{:.1}) reversals=({},{}) updates={} stable_steps={} failure_modes=(edge:{},model:{},axis_swap:{},stale:{}) timeout_blocker={} dominant_failure_mode={} best_within_success_tol={} latch_eligible={} best_age_steps={} online_beta=(pan+:{:.1},pan-:{:.1},tilt+:{:.1},tilt-:{:.1}) last_dt_sec={:.3} backend_hint={} best=({},{}) best_error=({},{}) trace=[{}]{}",
                    timeout_ms,
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
                    pan_tolerance,
                    tilt_tolerance,
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
                    timeout_blocker,
                    dominant_failure_mode,
                    best_within_success_tol,
                    latch_eligible,
                    best_age_steps,
                    pan_gain_tracker.positive_beta(),
                    pan_gain_tracker.negative_beta(),
                    tilt_gain_tracker.positive_beta(),
                    tilt_gain_tracker.negative_beta(),
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

        let dual_axis_active = pan_command_error.abs() > tolerance_count as f64
            && tilt_command_error.abs() > tolerance_count as f64;
        let dual_axis_close = dual_axis_active
            && pan_command_error.abs() <= TIE_BREAK_CLOSE_ERROR_COUNT
            && tilt_command_error.abs() <= TIE_BREAK_CLOSE_ERROR_COUNT;
        let mut step_error_abs = pan_command_error.abs().max(tilt_command_error.abs());
        let mut relative_command_applied = false;

        if fine_phase_candidate {
            let pan_relative_delta =
                relative_delta_from_error(pan_command_error, tolerance_count as f64);
            let tilt_relative_delta =
                relative_delta_from_error(tilt_command_error, tolerance_count as f64);
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
            let forced_secondary = forced_secondary_axis_command(
                &mut dual_axis_interleave,
                pan_command_error,
                tilt_command_error,
                tolerance_count as f64,
                pan_span,
                tilt_span,
            );
            let selected_command = forced_secondary.or_else(|| {
                command_from_errors(
                    pan_command_error,
                    tilt_command_error,
                    tolerance_count as f64,
                    tie_break_pan,
                    pan_span,
                    tilt_span,
                )
            });
            match selected_command {
                Some((direction, command_error_abs)) => {
                    step_error_abs = command_error_abs;
                    if axis_swap_lag_detected(
                        pan_command_error,
                        tilt_command_error,
                        direction,
                        pan_span,
                        tilt_span,
                    ) {
                        failure_modes.axis_swap_lag_hits =
                            failure_modes.axis_swap_lag_hits.saturating_add(1);
                    }
                    if edge_saturation_detected(
                        direction,
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
                    ) {
                        failure_modes.edge_saturation_hits =
                            failure_modes.edge_saturation_hits.saturating_add(1);
                    }
                    let speed = if near_target_speed1_mode {
                        1
                    } else {
                        speed_cap_for_error(command_error_abs).max(1)
                    };
                    let base_pulse_ms = if pulse_lut_candidate {
                        pulse_ms_for_direction_with_lut(
                            direction,
                            pan_command_error,
                            tilt_command_error,
                            &pan_pulse_lut,
                            &tilt_pulse_lut,
                            command_error_abs,
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
                    ptz_transport::move_ptz(client, channel, direction, speed, Some(pulse_ms))?;
                    pending_pulse_observation =
                        pending_pulse_observation_for_command(direction, pulse_ms);
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
                    if dual_axis_close {
                        tie_break_pan = !tie_break_pan;
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
            last_pan_u,
            last_tilt_u,
        )?;

        thread::sleep(Duration::from_millis(control_step_ms_for_error(
            step_error_abs,
        )));
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

fn axis_one_percent_threshold(span: f64) -> f64 {
    if !span.is_finite() || span <= f64::EPSILON {
        return 1.0;
    }
    (span * 0.01).floor().max(1.0)
}

fn normalized_axis_error(abs_error: f64, span: f64) -> f64 {
    if !abs_error.is_finite() || !span.is_finite() || span <= f64::EPSILON {
        return abs_error.abs();
    }
    abs_error.abs() / span
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

fn dominant_axis_by_normalized_error(
    pan_error: f64,
    tilt_error: f64,
    pan_span: f64,
    tilt_span: f64,
    dominance_ratio: f64,
) -> Option<ControlAxis> {
    let pan_norm = normalized_axis_error(pan_error, pan_span);
    let tilt_norm = normalized_axis_error(tilt_error, tilt_span);
    if pan_norm <= f64::EPSILON && tilt_norm <= f64::EPSILON {
        return None;
    }
    if pan_norm > tilt_norm * dominance_ratio {
        Some(ControlAxis::Pan)
    } else if tilt_norm > pan_norm * dominance_ratio {
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
) -> bool {
    let Some(dominant_axis) = dominant_axis_by_normalized_error(
        pan_command_error,
        tilt_command_error,
        pan_span,
        tilt_span,
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

fn command_from_errors(
    pan_error: f64,
    tilt_error: f64,
    tolerance_count: f64,
    tie_break_pan: bool,
    pan_span: f64,
    tilt_span: f64,
) -> Option<(PtzDirection, f64)> {
    let pan_active = pan_error.abs() > tolerance_count;
    let tilt_active = tilt_error.abs() > tolerance_count;
    if !pan_active && !tilt_active {
        return None;
    }
    if pan_active && tilt_active {
        let pan_abs = pan_error.abs();
        let tilt_abs = tilt_error.abs();
        let pan_norm = normalized_axis_error(pan_abs, pan_span);
        let tilt_norm = normalized_axis_error(tilt_abs, tilt_span);
        let prefer_pan = if pan_norm > tilt_norm * DUAL_AXIS_DOMINANCE_RATIO {
            true
        } else if tilt_norm > pan_norm * DUAL_AXIS_DOMINANCE_RATIO {
            false
        } else if pan_abs > TIE_BREAK_CLOSE_ERROR_COUNT || tilt_abs > TIE_BREAK_CLOSE_ERROR_COUNT {
            pan_norm >= tilt_norm
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

fn forced_secondary_axis_command(
    state: &mut DualAxisInterleaveState,
    pan_error: f64,
    tilt_error: f64,
    tolerance_count: f64,
    pan_span: f64,
    tilt_span: f64,
) -> Option<(PtzDirection, f64)> {
    let pan_active = pan_error.abs() > tolerance_count;
    let tilt_active = tilt_error.abs() > tolerance_count;
    if !pan_active || !tilt_active {
        state.dominant_axis = None;
        state.dominant_streak = 0;
        return None;
    }

    let pan_norm = normalized_axis_error(pan_error, pan_span);
    let tilt_norm = normalized_axis_error(tilt_error, tilt_span);
    if !pan_norm.is_finite() || !tilt_norm.is_finite() {
        state.dominant_axis = None;
        state.dominant_streak = 0;
        return None;
    }

    let (dominant_axis, dominant_norm, secondary_axis, secondary_norm, secondary_error) =
        if pan_norm >= tilt_norm {
            (
                ControlAxis::Pan,
                pan_norm,
                ControlAxis::Tilt,
                tilt_norm,
                tilt_error,
            )
        } else {
            (
                ControlAxis::Tilt,
                tilt_norm,
                ControlAxis::Pan,
                pan_norm,
                pan_error,
            )
        };

    if secondary_norm < SECONDARY_AXIS_INTERLEAVE_MIN_NORM {
        state.dominant_axis = Some(dominant_axis);
        state.dominant_streak = 0;
        return None;
    }

    if state.dominant_axis != Some(dominant_axis) {
        state.dominant_axis = Some(dominant_axis);
        state.dominant_streak = 0;
    }
    let interval = secondary_axis_interleave_interval(dominant_norm, secondary_norm);
    if state.dominant_streak < interval {
        state.dominant_streak = state.dominant_streak.saturating_add(1);
        return None;
    }

    state.dominant_streak = 0;
    command_for_axis_error(secondary_axis, secondary_error)
}

fn secondary_axis_interleave_interval(dominant_norm: f64, secondary_norm: f64) -> usize {
    if !dominant_norm.is_finite() || !secondary_norm.is_finite() || secondary_norm <= f64::EPSILON {
        return SECONDARY_AXIS_INTERLEAVE_INTERVAL_MAX;
    }
    let ratio = (dominant_norm / secondary_norm).clamp(1.0, 32.0);
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
    lut.pulse_ms_for_target(
        axis_direction,
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
    })
}

fn apply_pending_pulse_observation(
    pending: &mut Option<PendingPulseObservation>,
    pan_observed_delta: Option<f64>,
    tilt_observed_delta: Option<f64>,
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
        ControlAxis::Pan => {
            pan_lut.update(observation.direction, observation.pulse_ms, observed_delta)
        }
        ControlAxis::Tilt => {
            tilt_lut.update(observation.direction, observation.pulse_ms, observed_delta)
        }
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
        320
    } else if error_abs <= FINE_CONTROL_ERROR_COUNT {
        300
    } else if error_abs <= COARSE_CONTROL_ERROR_COUNT {
        260
    } else if error_abs <= 900.0 {
        230
    } else {
        250
    };
    base.max(SETTLE_STEP_MS)
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
        0.0
    } else {
        error
    }
}

fn dynamic_guard_deadband_ratio(deadband_hint_count: f64, tolerance_count: f64) -> f64 {
    if deadband_hint_count <= 0.0 {
        return CALIBRATION_GUARD_DEADBAND_RATIO;
    }
    let hint_ratio = (deadband_hint_count / tolerance_count.max(1.0)).clamp(0.5, 3.0);
    (CALIBRATION_GUARD_DEADBAND_RATIO + ((hint_ratio - 1.0) * 0.1)).clamp(0.25, 0.75)
}

fn update_reversal_counter(
    counter: &mut usize,
    previous_error: &mut Option<f64>,
    current_error: f64,
    tolerance_count: f64,
) {
    let previous = previous_error.unwrap_or(current_error);
    let detect_range =
        (tolerance_count * OSCILLATION_DETECT_RANGE_MULTIPLIER).max(OSCILLATION_MIN_DETECT_COUNT);

    let previous_in_band = previous.abs() > tolerance_count && previous.abs() <= detect_range;
    let current_in_band =
        current_error.abs() > tolerance_count && current_error.abs() <= detect_range;
    let sign_flipped = previous.signum() != current_error.signum()
        && previous.abs() > f64::EPSILON
        && current_error.abs() > f64::EPSILON;

    if sign_flipped && previous_in_band && current_in_band {
        *counter = counter.saturating_add(1);
    } else if current_error.abs() > detect_range {
        *counter = 0;
    } else {
        *counter = counter.saturating_sub(1);
    }
    *previous_error = Some(current_error);
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
    last_pan_u: f64,
    last_tilt_u: f64,
) -> AppResult<()> {
    let stored = StoredPtzEkfState {
        schema_version: EKF_STATE_SCHEMA_VERSION,
        state_key: state_key.to_string(),
        channel,
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

fn finalize_with_best_effort_stop<T>(
    client: &Client,
    channel: u8,
    result: AppResult<T>,
) -> AppResult<T> {
    let stop_error = ptz_transport::stop_ptz(client, channel).err();

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
        }
    }

    fn is_finite(&self) -> bool {
        let adaptive_r_ok = self.adaptive_r.is_none_or(|value| value.is_finite());
        let adaptive_q_ok = self.adaptive_q_scale.is_none_or(|value| value.is_finite());
        self.position.is_finite()
            && self.velocity.is_finite()
            && self.bias.is_finite()
            && adaptive_r_ok
            && adaptive_q_ok
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
        AxisOnlineGainTracker, DualAxisInterleaveState, FailureModeCounters,
        adaptive_axis_tolerance, apply_fine_phase_feedforward, apply_pending_pulse_observation,
        apply_reversal_guard, axis_count_bounds, axis_nominal_bounds, axis_one_percent_threshold,
        axis_swap_lag_detected, best_within_success_tolerance, clamp_tilt_edge_control,
        command_from_errors, control_axis_direction, control_pulse_ms_for_error,
        dominant_failure_mode_label, edge_saturation_detected, ekf_config,
        forced_secondary_axis_command, format_failure_mode_counters, load_stored_ekf_state,
        max_failure_mode_counters, model_mismatch_detected, near_target_speed1_pulse_ms,
        normalized_vector_error, parse_failure_mode_counters,
        pending_pulse_observation_for_command, position_stable_threshold_count,
        pulse_ms_for_direction_with_lut, relative_delta_from_error,
        required_stable_steps_for_oscillation, save_stored_ekf_state,
        secondary_axis_interleave_interval, select_control_error, should_retry_after_timeout,
        stale_status_detected, success_latch_ready, timeout_blocker_label, timeout_retry_budget_ms,
        update_reversal_counter,
    };
    use crate::app::usecases::ptz_controller::AxisEkf;
    use crate::app::usecases::ptz_pulse_lut::AxisPulseLut;
    use crate::app::usecases::ptz_settle_gate::completion_gate_allows_success;
    use crate::core::error::{AppError, ErrorKind};
    use crate::core::model::{AxisModelParams, NumericRange, PtzDirection};

    #[test]
    fn select_control_error_uses_measured_when_sign_conflicts() {
        let chosen = select_control_error(120.0, -40.0, 5);
        assert_eq!(chosen, -40.0);
    }

    #[test]
    fn axis_count_bounds_maps_range_and_applies_margin() {
        let range = NumericRange {
            min: -1000,
            max: 2000,
        };
        let (min_count, max_count) = axis_count_bounds(Some(&range), None, 3000, -3000.0, 3000.0);
        assert!((min_count - -1120.0).abs() < 1e-6);
        assert!((max_count - 3120.0).abs() < 1e-6);
    }

    #[test]
    fn axis_nominal_bounds_prefers_calibration_without_margin() {
        let (min_count, max_count) = axis_nominal_bounds(None, Some((0, 1240)), -1800.0, 1800.0);
        assert_eq!(min_count, 0.0);
        assert_eq!(max_count, 1240.0);

        let range = NumericRange { min: 10, max: 100 };
        let (range_min, range_max) = axis_nominal_bounds(Some(&range), None, -1800.0, 1800.0);
        assert_eq!(range_min, 10.0);
        assert_eq!(range_max, 100.0);
    }

    #[test]
    fn command_from_errors_prioritizes_dominant_axis_and_uses_tie_break() {
        let dominant = command_from_errors(220.0, -100.0, 10.0, true, 1000.0, 1000.0)
            .expect("command should be produced");
        assert_eq!(dominant.0, PtzDirection::Right);

        let tie_break_pan = command_from_errors(120.0, -110.0, 10.0, true, 1000.0, 1000.0)
            .expect("command should be produced");
        assert_eq!(tie_break_pan.0, PtzDirection::Right);

        let tie_break_tilt = command_from_errors(120.0, -110.0, 10.0, false, 1000.0, 1000.0)
            .expect("command should be produced");
        assert_eq!(tie_break_tilt.0, PtzDirection::Down);

        let single_axis = command_from_errors(0.0, -110.0, 10.0, true, 1000.0, 1000.0)
            .expect("command should be produced");
        assert_eq!(single_axis.0, PtzDirection::Down);
        assert!(dominant.1 >= 1.0);
    }

    #[test]
    fn command_from_errors_uses_normalized_axis_priority() {
        assert_eq!(
            command_from_errors(200.0, 100.0, 10.0, true, 7360.0, 1240.0).map(|cmd| cmd.0),
            Some(PtzDirection::Up)
        );
        assert_eq!(
            command_from_errors(8.0, 8.0, 10.0, true, 7360.0, 1240.0),
            None
        );
    }

    #[test]
    fn secondary_axis_interleave_interval_scales_with_ratio() {
        assert_eq!(secondary_axis_interleave_interval(0.30, 0.25), 1);
        assert_eq!(secondary_axis_interleave_interval(0.30, 0.18), 2);
        assert_eq!(secondary_axis_interleave_interval(0.30, 0.12), 3);
        assert_eq!(secondary_axis_interleave_interval(0.30, 0.08), 4);
        assert_eq!(secondary_axis_interleave_interval(0.30, 0.04), 5);
    }

    #[test]
    fn forced_secondary_axis_command_alternates_for_balanced_dual_axis_error() {
        let mut state = DualAxisInterleaveState::default();
        let mut directions = Vec::new();
        for _ in 0..4 {
            let forced =
                forced_secondary_axis_command(&mut state, -3_422.0, 476.0, 12.0, 7_360.0, 1_240.0)
                    .map(|(direction, _)| direction);
            directions.push(forced);
        }

        assert_eq!(directions[0], None);
        assert_eq!(directions[1], Some(PtzDirection::Up));
        assert_eq!(directions[2], None);
        assert_eq!(directions[3], Some(PtzDirection::Up));

        let reset = forced_secondary_axis_command(&mut state, -80.0, 0.0, 12.0, 7_360.0, 1_240.0);
        assert_eq!(reset, None);
        let first_after_reset =
            forced_secondary_axis_command(&mut state, -400.0, 200.0, 12.0, 7_360.0, 1_240.0);
        assert_eq!(first_after_reset, None);
    }

    #[test]
    fn one_percent_and_vector_helpers_are_finite() {
        assert_eq!(axis_one_percent_threshold(7360.0), 73.0);
        assert_eq!(axis_one_percent_threshold(1240.0), 12.0);
        let vector = normalized_vector_error(73.0, 12.0, 7360.0, 1240.0);
        assert!(vector.is_finite());
        assert!(vector > 0.0);
    }

    #[test]
    fn failure_mode_helpers_detect_expected_patterns() {
        assert!(model_mismatch_detected(120.0, Some(10.0)));
        assert!(!model_mismatch_detected(10.0, Some(1.0)));

        let mut stale = 0usize;
        assert!(!stale_status_detected(
            0.6,
            0.0,
            Some(0.1),
            Some(0.1),
            &mut stale
        ));
        assert!(stale_status_detected(
            0.6,
            0.0,
            Some(0.1),
            Some(0.1),
            &mut stale
        ));

        assert!(axis_swap_lag_detected(
            500.0,
            40.0,
            PtzDirection::Up,
            7360.0,
            1240.0
        ));
        assert!(!axis_swap_lag_detected(
            40.0,
            500.0,
            PtzDirection::Up,
            7360.0,
            1240.0
        ));

        assert!(edge_saturation_detected(
            PtzDirection::Up,
            7000.0,
            0.0,
            7360.0,
            7360.0,
            0.0,
            73.0,
            1235.0,
            0.0,
            1240.0,
            40.0,
            12.0,
        ));
        assert!(!edge_saturation_detected(
            PtzDirection::Right,
            2000.0,
            0.0,
            7360.0,
            7360.0,
            40.0,
            73.0,
            600.0,
            0.0,
            1240.0,
            0.0,
            12.0,
        ));
    }

    #[test]
    fn failure_mode_counter_parsing_and_max_are_stable() {
        let parsed = parse_failure_mode_counters(
            "set_absolute_raw timeout ... failure_modes=(edge:7,model:3,axis_swap:0,stale:1)",
        )
        .expect("failure mode tuple should parse");
        assert_eq!(
            parsed,
            FailureModeCounters {
                edge_saturation_hits: 7,
                model_mismatch_hits: 3,
                axis_swap_lag_hits: 0,
                stale_status_hits: 1,
            }
        );
        assert_eq!(
            format_failure_mode_counters(parsed),
            "(edge:7,model:3,axis_swap:0,stale:1)"
        );

        let retry = FailureModeCounters {
            edge_saturation_hits: 5,
            model_mismatch_hits: 9,
            axis_swap_lag_hits: 2,
            stale_status_hits: 0,
        };
        let merged = max_failure_mode_counters(parsed, retry);
        assert_eq!(
            merged,
            FailureModeCounters {
                edge_saturation_hits: 7,
                model_mismatch_hits: 9,
                axis_swap_lag_hits: 2,
                stale_status_hits: 1,
            }
        );
        assert_eq!(dominant_failure_mode_label(merged), "model");
    }

    #[test]
    fn online_gain_tracker_updates_directionally_and_rejects_outliers() {
        let mut tracker = AxisOnlineGainTracker::seeded(120.0);
        tracker.observe(0.5, Some(90.0), 7_360.0);
        assert!(tracker.positive_beta() > 120.0);
        assert!((tracker.negative_beta() - 120.0).abs() < 1e-6);

        tracker.observe(-0.6, Some(-30.0), 7_360.0);
        assert!(tracker.negative_beta() < 120.0);

        let before = tracker.positive_beta();
        tracker.observe(0.4, Some(2_000.0), 7_360.0);
        assert!((tracker.positive_beta() - before).abs() < 1e-6);
    }

    #[test]
    fn success_latch_helpers_gate_by_age_and_motion() {
        let idle = Some(crate::app::usecases::ptz_transport::TransportMotionHint {
            moving: Some(false),
            move_age_ms: Some(300),
        });
        let moving = Some(crate::app::usecases::ptz_transport::TransportMotionHint {
            moving: Some(true),
            move_age_ms: Some(10),
        });
        assert!(success_latch_ready(1, 0.02, idle));
        assert!(!success_latch_ready(1, 0.005, moving));
        assert!(success_latch_ready(0, 0.01, idle));
        assert!(!success_latch_ready(0, 0.08, idle));

        assert!(best_within_success_tolerance(
            super::BestObservedState {
                pan_count: 100,
                tilt_count: 20,
                pan_abs_error: 15,
                tilt_abs_error: 8,
            },
            20.0,
            10.0
        ));
        assert_eq!(
            timeout_blocker_label(false, false, true, false),
            "latch_gate"
        );
    }

    #[test]
    fn timeout_retry_budget_is_bounded_and_scaled() {
        assert_eq!(timeout_retry_budget_ms(9_000), 27_000);
        assert_eq!(timeout_retry_budget_ms(25_000), 36_000);
        assert_eq!(timeout_retry_budget_ms(60_000), 36_000);
    }

    #[test]
    fn should_retry_after_timeout_matches_timeout_errors_only() {
        let timeout_error = AppError::new(
            ErrorKind::UnexpectedResponse,
            "set_absolute_raw timeout after 25000ms",
        );
        assert!(should_retry_after_timeout(&timeout_error));

        let other_unexpected = AppError::new(ErrorKind::UnexpectedResponse, "other failure");
        assert!(!should_retry_after_timeout(&other_unexpected));

        let network = AppError::new(ErrorKind::Network, "set_absolute_raw timeout after 25000ms");
        assert!(!should_retry_after_timeout(&network));
    }

    #[test]
    fn required_stable_steps_reduces_when_oscillation_is_high() {
        assert_eq!(required_stable_steps_for_oscillation(0, 0), 2);
        assert_eq!(required_stable_steps_for_oscillation(2, 3), 2);
        assert_eq!(required_stable_steps_for_oscillation(4, 2), 1);
    }

    #[test]
    fn clamp_tilt_edge_control_limits_edge_risk_only() {
        assert_eq!(
            clamp_tilt_edge_control(PtzDirection::Up, 4, 90, 1_230.0, 0.0, 1_240.0),
            (1, 20)
        );
        assert_eq!(
            clamp_tilt_edge_control(PtzDirection::Down, 3, 80, 5.0, 0.0, 1_240.0),
            (1, 20)
        );
        assert_eq!(
            clamp_tilt_edge_control(PtzDirection::Down, 3, 80, 600.0, 0.0, 1_240.0),
            (3, 80)
        );
        assert_eq!(
            clamp_tilt_edge_control(PtzDirection::Right, 4, 90, 1_230.0, 0.0, 1_240.0),
            (4, 90)
        );
    }

    #[test]
    fn apply_reversal_guard_blocks_small_reverse_commands() {
        let blocked = apply_reversal_guard(-30.0, 0.5, 20.0, 0.0);
        assert!(blocked < -5.0 && blocked > -30.0);

        let allowed = apply_reversal_guard(-220.0, 0.5, 20.0, 0.0);
        assert_eq!(allowed, -220.0);

        let blocked_by_deadband = apply_reversal_guard(-70.0, 0.5, 10.0, 180.0);
        assert_eq!(blocked_by_deadband, 0.0);
    }

    #[test]
    fn apply_fine_phase_feedforward_is_bounded_and_biases_command() {
        let boosted = apply_fine_phase_feedforward(60.0, 120.0, 0.6, Some(20.0), 20.0);
        assert!(boosted > 60.0);

        let bounded = apply_fine_phase_feedforward(20.0, 600.0, 1.0, Some(-400.0), 20.0);
        assert!(bounded <= 64.0);
    }

    #[test]
    fn relative_delta_from_error_uses_tolerance_window() {
        assert_eq!(relative_delta_from_error(8.0, 10.0), 0);
        assert_eq!(relative_delta_from_error(18.0, 10.0), 10);
        assert_eq!(relative_delta_from_error(-45.0, 10.0), -25);
    }

    #[test]
    fn completion_gate_respects_backend_motion_hint() {
        assert!(!completion_gate_allows_success(None, None, 120, 2, 2));
        assert!(!completion_gate_allows_success(
            Some(true),
            Some(250),
            120,
            2,
            2
        ));
        assert!(!completion_gate_allows_success(
            Some(false),
            Some(70),
            120,
            2,
            2
        ));
        assert!(completion_gate_allows_success(
            Some(false),
            Some(260),
            120,
            2,
            2
        ));
    }

    #[test]
    fn pulse_lut_path_produces_short_pulse_for_small_error() {
        let pan_lut = AxisPulseLut::seeded(120.0);
        let tilt_lut = AxisPulseLut::seeded(120.0);
        let pulse = pulse_ms_for_direction_with_lut(
            PtzDirection::Right,
            90.0,
            0.0,
            &pan_lut,
            &tilt_lut,
            90.0,
        );
        assert!(pulse >= 10);
        assert!(pulse <= 140);
    }

    #[test]
    fn pending_pulse_observation_updates_axis_lut() {
        let mut pan_lut = AxisPulseLut::seeded(120.0);
        let mut tilt_lut = AxisPulseLut::seeded(120.0);
        let mut pending = pending_pulse_observation_for_command(PtzDirection::Right, 40);
        apply_pending_pulse_observation(
            &mut pending,
            Some(120.0),
            Some(0.0),
            &mut pan_lut,
            &mut tilt_lut,
        );
        assert!(pending.is_none());
        assert!(
            pan_lut.counts_per_ms(crate::app::usecases::ptz_pulse_lut::AxisDirection::Positive)
                > 1.0
        );
    }

    #[test]
    fn position_stable_threshold_accounts_for_deadband() {
        let threshold = position_stable_threshold_count(10.0, 180.0, 20.0);
        assert!(threshold >= 10.0);
        assert!(threshold <= 24.0);
    }

    #[test]
    fn control_axis_direction_rejects_diagonal() {
        assert!(control_axis_direction(PtzDirection::LeftUp).is_none());
        assert!(control_axis_direction(PtzDirection::RightDown).is_none());
    }

    #[test]
    fn near_target_speed1_pulse_ms_allows_tilt_micro_and_clamps_upper_bound() {
        assert_eq!(near_target_speed1_pulse_ms(24, 32.0, PtzDirection::Up), 0);
        assert_eq!(near_target_speed1_pulse_ms(24, 60.0, PtzDirection::Up), 24);
        assert_eq!(near_target_speed1_pulse_ms(0, 24.0, PtzDirection::Right), 0);
        assert_eq!(
            near_target_speed1_pulse_ms(90, 120.0, PtzDirection::Right),
            45
        );
    }

    #[test]
    fn update_reversal_counter_detects_near_target_sign_flips() {
        let mut counter = 0usize;
        let mut previous = None;
        update_reversal_counter(&mut counter, &mut previous, 140.0, 50.0);
        update_reversal_counter(&mut counter, &mut previous, -130.0, 50.0);
        assert_eq!(counter, 1);
        update_reversal_counter(&mut counter, &mut previous, 800.0, 50.0);
        assert_eq!(counter, 0);
    }

    #[test]
    fn adaptive_axis_tolerance_relaxes_after_repeated_reversals() {
        assert_eq!(adaptive_axis_tolerance(50.0, 0, 120.0), 50.0);
        assert_eq!(adaptive_axis_tolerance(50.0, 4, 200.0), 94.0);
        assert_eq!(adaptive_axis_tolerance(180.0, 8, 200.0), 224.0);
    }

    #[test]
    fn control_pulse_ms_uses_micro_pulses_near_target() {
        assert_eq!(control_pulse_ms_for_error(80.0), 0);
        assert_eq!(control_pulse_ms_for_error(150.0), 20);
        assert_eq!(control_pulse_ms_for_error(300.0), 35);
    }

    #[test]
    fn ekf_state_roundtrip_save_and_load() {
        let temp_file = std::env::temp_dir().join(format!(
            "reocli-ekf-count-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let channel = 1u8;
        let state_key = "camera-a.ch1";
        let model = AxisModelParams {
            alpha: 0.9,
            beta: 120.0,
        };
        let mut pan_filter = AxisEkf::new(ekf_config(-3000.0, 3000.0), model, 1200.0);
        let mut tilt_filter = AxisEkf::new(ekf_config(-1500.0, 1500.0), model, -200.0);
        let _ = pan_filter.update(0.3, 1230.0);
        let _ = tilt_filter.update(-0.2, -180.0);

        save_stored_ekf_state(
            &temp_file,
            state_key,
            channel,
            &pan_filter,
            &tilt_filter,
            0.25,
            -0.5,
        )
        .expect("EKF state save should succeed");

        let loaded = load_stored_ekf_state(&temp_file, state_key, channel)
            .expect("EKF state load should succeed")
            .expect("EKF state should exist");
        assert_eq!(loaded.channel, channel);
        assert_eq!(loaded.state_key, state_key);
        assert!((loaded.last_pan_u - 0.25).abs() < 1e-9);
        assert!((loaded.last_tilt_u + 0.5).abs() < 1e-9);
        assert!(loaded.pan.position.is_finite());
        assert!(loaded.tilt.position.is_finite());

        let _ = fs::remove_file(PathBuf::from(&temp_file));
    }
}
