use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use mockito::{Matcher, Server};

use super::{
    AxisKind, AxisMotion, CALIBRATION_MODEL_TRIM_RATIO, CALIBRATION_SCHEMA_VERSION,
    CALIBRATION_SOURCE_MEASURED, DirectionalDeadband, MODEL_ALPHA_MAX, MODEL_ALPHA_MIN,
    MODEL_BETA_MAX, MODEL_BETA_MIN, StoredCalibration, attempt_home_restore_on_failure,
    axis_model_min_samples, axis_model_sample_cap, axis_quality_threshold_count,
    axis_sample_weights, build_axis_count_range, calibration_control_u,
    calibration_effective_ts_sec, calibration_min_move_delta, calibration_pulse_ms,
    calibration_pulse_speed, calibration_stall_delta, can_reuse_saved_calibration,
    deadband_upper_bound_for_span, estimate_deadband_from_samples, estimate_model_from_samples,
    estimate_model_from_sweep, estimate_model_from_sweep_with_quality, evenly_spaced_samples,
    execute, fallback_model_for_span, map_status_to_counts, robust_deadband_from_samples,
    save_stored_calibration, validate_measured_calibration_quality, winsorize_samples,
};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{
    AxisModelParams, CalibrationParams, DeviceInfo, NumericRange, PtzDirection, PtzStatus,
};
use crate::interfaces::runtime;
use crate::reolink::client::{Auth, Client};

#[test]
fn execute_requires_channel_match_for_saved_reuse() {
    let unique = unique_suffix();
    let device_info = DeviceInfo {
        model: format!("model-{unique}"),
        firmware: format!("fw-{unique}"),
        serial_number: format!("serial-{unique}"),
    };
    let calibration_path = runtime::calibration_file_path_for_camera(&device_info);
    cleanup_file(&calibration_path);
    let saved = StoredCalibration {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        source: CALIBRATION_SOURCE_MEASURED.to_string(),
        camera_key: runtime::calibration_camera_key(&device_info),
        channel: 1,
        calibration: sample_calibration_params(&device_info),
    };
    save_stored_calibration(&calibration_path, &saved)
        .expect("saved calibration should be created");

    let mut server = Server::new();
    let _dev_info_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{"cmd":"GetDevInfo","code":0,"value":{{"DevInfo":{{"model":"{}","firmware":"{}","serial":"{}"}}}}}}]"#,
            device_info.model, device_info.firmware, device_info.serial_number
        ))
        .expect(1)
        .create();
    let _cur_pos_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCurPos".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzCurPos","code":0,
                "value":{"PtzCurPos":{"channel":0,"Ppos":120,"Tpos":40}},
                "range":{"PtzCurPos":{"Ppos":{"min":-3550,"max":3550},"Tpos":{"min":0,"max":900}}}
            }]"#,
        )
        .expect_at_least(6)
        .expect_at_most(24)
        .create();
    let _zoom_focus_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetZoomFocus".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetZoomFocus","code":1}]"#)
        .expect(1)
        .create();
    let _preset_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetPtzPreset","code":1}]"#)
        .expect(1)
        .create();
    let _check_state_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCheckState".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetPtzCheckState","code":0,"value":{"PtzCheckState":2}}]"#)
        .expect(1)
        .create();
    let _ptz_ctrl_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect_at_least(9)
        .expect_at_most(24)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let report = execute(&client, 0).expect("calibration should fallback and succeed");
    assert!(!report.reused_existing);
    assert_eq!(report.channel, 0);
    assert_eq!(report.params.channel, 0);
    assert_eq!(
        report.params.camera_key,
        runtime::calibration_camera_key(&device_info)
    );

    cleanup_file(Path::new(&report.calibration_path));
}

#[test]
fn can_reuse_saved_calibration_requires_matching_channel() {
    let device_info = DeviceInfo {
        model: "model".to_string(),
        firmware: "firmware".to_string(),
        serial_number: "serial".to_string(),
    };
    let saved = StoredCalibration {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        source: CALIBRATION_SOURCE_MEASURED.to_string(),
        camera_key: runtime::calibration_camera_key(&device_info),
        channel: 3,
        calibration: sample_calibration_params(&device_info),
    };
    let calibrated = PtzStatus {
        calibration_state: Some(2),
        ..PtzStatus::default()
    };
    let not_calibrated = PtzStatus {
        calibration_state: Some(1),
        ..PtzStatus::default()
    };

    assert!(can_reuse_saved_calibration(&calibrated, 3, &saved));
    assert!(!can_reuse_saved_calibration(&calibrated, 1, &saved));
    assert!(!can_reuse_saved_calibration(&not_calibrated, 3, &saved));
}

#[test]
fn attempt_home_restore_on_failure_restores_known_axes() {
    let measured: AppResult<CalibrationParams> = Err(AppError::new(
        ErrorKind::UnexpectedResponse,
        "forced failure",
    ));
    let pan_motion = AxisMotion {
        increase: PtzDirection::Right,
        decrease: PtzDirection::Left,
    };
    let tilt_motion = AxisMotion {
        increase: PtzDirection::Up,
        decrease: PtzDirection::Down,
    };

    let mut calls = Vec::new();
    attempt_home_restore_on_failure(
        &measured,
        120,
        -45,
        Some(pan_motion),
        Some(tilt_motion),
        |axis, home_position, motion| {
            calls.push((axis, home_position, motion.increase, motion.decrease));
        },
    );

    assert_eq!(calls.len(), 2);
    assert!(matches!(
        calls[0],
        (AxisKind::Pan, 120, PtzDirection::Right, PtzDirection::Left)
    ));
    assert!(matches!(
        calls[1],
        (AxisKind::Tilt, -45, PtzDirection::Up, PtzDirection::Down)
    ));
}

#[test]
fn attempt_home_restore_on_failure_skips_when_measured_succeeds() {
    let measured = Ok(CalibrationParams::default());
    let mut called = false;
    attempt_home_restore_on_failure(
        &measured,
        120,
        -45,
        Some(AxisMotion {
            increase: PtzDirection::Right,
            decrease: PtzDirection::Left,
        }),
        Some(AxisMotion {
            increase: PtzDirection::Up,
            decrease: PtzDirection::Down,
        }),
        |_axis, _home_position, _motion| {
            called = true;
        },
    );
    assert!(!called);
}

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

#[test]
fn pan_weighted_estimation_matches_unweighted_reference_formula() {
    let samples = [80.0, 97.0, 108.0, 114.0, 118.0, 122.0];
    let fallback = fallback_model_for_span(7_200.0);
    let weighted = estimate_model_from_samples(AxisKind::Pan, &samples, fallback);
    let reference = unweighted_reference_model(AxisKind::Pan, &samples, fallback);

    assert_approx_eq(weighted.alpha, reference.alpha, 1e-12);
    assert_approx_eq(weighted.beta, reference.beta, 1e-9);
}

#[test]
fn tilt_axis_weights_downweight_far_outlier_samples() {
    let samples = [40.0, 41.0, 42.0, 43.0, 180.0, 44.0, 45.0];
    let pan_weights = axis_sample_weights(AxisKind::Pan, &samples);
    let tilt_weights = axis_sample_weights(AxisKind::Tilt, &samples);

    assert!(
        pan_weights
            .iter()
            .all(|weight| (*weight - 1.0).abs() <= f64::EPSILON)
    );
    assert!(tilt_weights[4] < tilt_weights[3]);
    assert!(tilt_weights[4] <= 0.2);
}

#[test]
fn weighted_tilt_estimation_is_more_robust_to_noisy_sequences() {
    let clean = synthetic_axis_samples(AxisKind::Tilt, 0.9, 120.0, 85.0, 96);
    let mut noisy = clean.clone();
    for (index, sample) in noisy.iter_mut().enumerate() {
        if index % 9 == 0 {
            *sample *= 2.2;
        } else if index % 11 == 0 {
            *sample *= 0.55;
        }
    }

    let fallback = fallback_model_for_span(1_800.0);
    let stabilized_noisy = winsorize_samples(&noisy, CALIBRATION_MODEL_TRIM_RATIO);
    let weighted_noisy = estimate_model_from_samples(AxisKind::Tilt, &stabilized_noisy, fallback);
    let unweighted_noisy = unweighted_reference_model(AxisKind::Tilt, &stabilized_noisy, fallback);
    let clean_reference = estimate_model_from_samples(AxisKind::Tilt, &clean, fallback);
    let weighted_distance = normalized_model_distance(weighted_noisy, clean_reference);
    let unweighted_distance = normalized_model_distance(unweighted_noisy, clean_reference);

    assert!(
        weighted_distance < unweighted_distance,
        "expected weighted tilt fit to be closer to clean-model fit: weighted={weighted_distance}, unweighted={unweighted_distance}"
    );
    assert!((MODEL_ALPHA_MIN..=MODEL_ALPHA_MAX).contains(&weighted_noisy.alpha));
    assert!((MODEL_BETA_MIN..=MODEL_BETA_MAX).contains(&weighted_noisy.beta));
}

#[test]
fn estimate_model_from_sweep_with_quality_applies_fallback_blend_for_noisy_samples() {
    let estimate = estimate_model_from_sweep_with_quality(
        AxisKind::Pan,
        7_200.0,
        &[10.0, 10.0, 10.0, 300.0, 10.0, 10.0, 10.0, 300.0],
    );
    assert!(estimate.fallback_blend_ratio > 0.0);
    assert!(estimate.residual_p95_count > 0);
}

#[test]
fn axis_model_sample_policy_biases_tilt_higher_than_pan() {
    let pan_cap = axis_model_sample_cap(AxisKind::Pan);
    let tilt_cap = axis_model_sample_cap(AxisKind::Tilt);
    let pan_min = axis_model_min_samples(AxisKind::Pan);
    let tilt_min = axis_model_min_samples(AxisKind::Tilt);

    assert!(tilt_cap > pan_cap);
    assert!(tilt_min > pan_min);
    assert!(pan_min <= pan_cap);
    assert!(tilt_min <= tilt_cap);
}

#[test]
fn estimate_model_from_sweep_with_quality_uses_axis_specific_sample_caps() {
    let source = (1..=240).map(|value| value as f64).collect::<Vec<_>>();
    let pan_estimate = estimate_model_from_sweep_with_quality(AxisKind::Pan, 7_200.0, &source);
    let tilt_estimate = estimate_model_from_sweep_with_quality(AxisKind::Tilt, 1_800.0, &source);

    assert_eq!(
        pan_estimate.sample_count,
        axis_model_sample_cap(AxisKind::Pan)
    );
    assert_eq!(
        tilt_estimate.sample_count,
        axis_model_sample_cap(AxisKind::Tilt)
    );
    assert!(tilt_estimate.sample_count > pan_estimate.sample_count);
}

#[test]
fn winsorize_samples_clamps_large_outliers() {
    let winsorized = winsorize_samples(&[10.0, 11.0, 12.0, 800.0, 13.0, 14.0], 0.1);
    assert_eq!(winsorized.len(), 6);
    assert!(winsorized[3] < 800.0);
}

#[test]
fn tilt_calibration_pulse_profile_is_finer_than_pan() {
    assert!(calibration_pulse_speed(AxisKind::Tilt) < calibration_pulse_speed(AxisKind::Pan));
    assert!(calibration_pulse_ms(AxisKind::Tilt) < calibration_pulse_ms(AxisKind::Pan));
    assert!(
        calibration_min_move_delta(AxisKind::Tilt) <= calibration_min_move_delta(AxisKind::Pan)
    );
    assert!(calibration_stall_delta(AxisKind::Tilt) <= calibration_stall_delta(AxisKind::Pan));
}

#[test]
fn evenly_spaced_samples_caps_to_target_count() {
    let source = (0..100).map(|value| value as f64).collect::<Vec<_>>();
    let sampled = evenly_spaced_samples(&source, 50);
    assert_eq!(sampled.len(), 50);
    assert_eq!(sampled.first().copied(), Some(0.0));
    assert_eq!(sampled.last().copied(), Some(99.0));
}

#[test]
fn robust_deadband_from_samples_resists_large_outliers() {
    let estimate =
        robust_deadband_from_samples(&[7, 8, 7, 6, 140, 7, 8]).expect("estimate should exist");
    assert_eq!(estimate, 7);
}

#[test]
fn estimate_deadband_from_samples_clips_to_span_upper_bound() {
    let span = 1_000.0;
    let clipped = estimate_deadband_from_samples(&[240, 250, 260, 270, 280], span);
    assert_eq!(clipped, deadband_upper_bound_for_span(span));
}

#[test]
fn measured_quality_gate_accepts_values_on_axis_thresholds() {
    let pan_span = 7_200.0;
    let tilt_span = 1_800.0;
    let pan_threshold = axis_quality_threshold_count(AxisKind::Pan, pan_span);
    let tilt_threshold = axis_quality_threshold_count(AxisKind::Tilt, tilt_span);

    validate_measured_calibration_quality(pan_span, tilt_span, pan_threshold, tilt_threshold)
        .expect("threshold-edge values should be accepted");
}

#[test]
fn measured_quality_gate_rejects_when_pan_p95_exceeds_threshold_by_one() {
    let pan_span = 7_200.0;
    let tilt_span = 1_800.0;
    let pan_threshold = axis_quality_threshold_count(AxisKind::Pan, pan_span);
    let tilt_threshold = axis_quality_threshold_count(AxisKind::Tilt, tilt_span);

    let error = validate_measured_calibration_quality(
        pan_span,
        tilt_span,
        pan_threshold + 1,
        tilt_threshold,
    )
    .expect_err("pan above threshold should be rejected");
    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("pan_p95="));
    assert!(error.message.contains("tilt_p95="));
    assert!(error.message.contains(&format!("max={}", pan_threshold)));
}

#[test]
fn measured_quality_gate_rejects_when_tilt_p95_exceeds_threshold_by_one() {
    let pan_span = 7_200.0;
    let tilt_span = 1_800.0;
    let pan_threshold = axis_quality_threshold_count(AxisKind::Pan, pan_span);
    let tilt_threshold = axis_quality_threshold_count(AxisKind::Tilt, tilt_span);

    let error = validate_measured_calibration_quality(
        pan_span,
        tilt_span,
        pan_threshold,
        tilt_threshold + 1,
    )
    .expect_err("tilt above threshold should be rejected");
    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains(&format!("max={}", tilt_threshold)));
    assert!(error.message.contains("ratio="));
}

#[test]
fn directional_deadband_compatibility_uses_max_direction() {
    let directional = DirectionalDeadband {
        increase_count: 9,
        decrease_count: 14,
    };
    assert_eq!(directional.compatibility_count(), 14);
}

fn assert_approx_eq(lhs: f64, rhs: f64, tolerance: f64) {
    let delta = (lhs - rhs).abs();
    assert!(
        delta <= tolerance,
        "values differ: lhs={lhs}, rhs={rhs}, delta={delta}, tolerance={tolerance}"
    );
}

fn normalized_model_distance(lhs: AxisModelParams, rhs: AxisModelParams) -> f64 {
    let alpha_range = (MODEL_ALPHA_MAX - MODEL_ALPHA_MIN).max(f64::EPSILON);
    let beta_range = (MODEL_BETA_MAX - MODEL_BETA_MIN).max(f64::EPSILON);
    ((lhs.alpha - rhs.alpha).abs() / alpha_range) + ((lhs.beta - rhs.beta).abs() / beta_range)
}

fn synthetic_axis_samples(
    axis: AxisKind,
    alpha: f64,
    beta: f64,
    start: f64,
    count: usize,
) -> Vec<f64> {
    let input_gain = beta * calibration_control_u(axis) * calibration_effective_ts_sec(axis);
    let mut current = start;
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        samples.push(current);
        current = alpha * current + input_gain;
    }
    samples
}

fn unweighted_reference_model(
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

fn sample_calibration_params(device_info: &DeviceInfo) -> CalibrationParams {
    CalibrationParams {
        serial_number: device_info.serial_number.clone(),
        model: device_info.model.clone(),
        firmware: device_info.firmware.clone(),
        pan_min_count: -3550,
        pan_max_count: 3550,
        pan_deadband_count: 6,
        pan_deadband_increase_count: Some(6),
        pan_deadband_decrease_count: Some(6),
        tilt_min_count: 0,
        tilt_max_count: 900,
        tilt_deadband_count: 6,
        tilt_deadband_increase_count: Some(6),
        tilt_deadband_decrease_count: Some(6),
        pan_model: AxisModelParams {
            alpha: 0.9,
            beta: 120.0,
        },
        tilt_model: AxisModelParams {
            alpha: 0.9,
            beta: 60.0,
        },
        created_at: "1".to_string(),
    }
}

fn unique_suffix() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{now}-{}", std::process::id())
}

fn cleanup_file(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}
