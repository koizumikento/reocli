use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    AxisOnlineGainTracker, DualAxisInterleaveState, FailureModeCounters, adaptive_axis_tolerance,
    apply_fine_phase_feedforward, apply_pending_pulse_observation, apply_reversal_guard,
    axis_count_bounds, axis_one_percent_threshold, axis_swap_lag_detected,
    best_stagnation_near_miss_eligible, best_within_success_tolerance,
    calibrated_success_tolerance, clamp_tilt_edge_control, command_from_errors,
    control_axis_direction, control_pulse_ms_for_error, dominant_failure_mode_label,
    edge_saturation_detected, ekf_config, enforce_residual_command_activity,
    forced_secondary_axis_command, format_failure_mode_counters, load_stored_ekf_state,
    max_failure_mode_counters, model_mismatch_detected, near_target_speed1_pulse_ms,
    normalized_vector_error, oscillation_damping_active, parse_failure_mode_counters,
    parse_onvif_duration_ms, pending_pulse_observation_for_command,
    position_stable_threshold_count, pulse_ms_for_direction_with_lut, relative_delta_from_error,
    required_stable_steps_for_oscillation, save_stored_ekf_state,
    secondary_axis_interleave_interval, select_control_error, should_force_cgi_for_onvif_options,
    should_retry_after_timeout, stale_status_detected, success_latch_ready,
    success_latch_stagnation_ready, timeout_blocker_label, timeout_retry_budget_ms,
    update_reversal_counter,
};
use crate::app::usecases::ptz_controller::AxisEkf;
use crate::app::usecases::ptz_pulse_lut::AxisPulseLut;
use crate::app::usecases::ptz_settle_gate::completion_gate_allows_success;
use crate::core::error::{AppError, ErrorKind};
use crate::core::model::{AxisModelParams, NumericRange, PtzDirection};
use crate::reolink::onvif::OnvifPtzConfigurationOptions;

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
fn command_from_errors_prioritizes_dominant_axis_and_uses_tie_break() {
    let dominant = command_from_errors(220.0, -100.0, 10.0, true, 1000.0, 1000.0, 120.0, 68.0)
        .expect("command should be produced");
    assert_eq!(dominant.0, PtzDirection::Right);

    let tie_break_pan = command_from_errors(120.0, -68.0, 10.0, true, 1000.0, 1000.0, 120.0, 68.0)
        .expect("command should be produced");
    assert_eq!(tie_break_pan.0, PtzDirection::Right);

    let tie_break_tilt =
        command_from_errors(120.0, -68.0, 10.0, false, 1000.0, 1000.0, 120.0, 68.0)
            .expect("command should be produced");
    assert_eq!(tie_break_tilt.0, PtzDirection::Down);

    let single_axis = command_from_errors(0.0, -110.0, 10.0, true, 1000.0, 1000.0, 120.0, 68.0)
        .expect("command should be produced");
    assert_eq!(single_axis.0, PtzDirection::Down);
    assert!(dominant.1 >= 1.0);
}

#[test]
fn command_from_errors_uses_success_tolerance_priority_for_asymmetric_span() {
    assert_eq!(
        command_from_errors(200.0, 100.0, 10.0, true, 7360.0, 1240.0, 120.0, 68.0).map(|cmd| cmd.0),
        Some(PtzDirection::Right)
    );
    assert_eq!(
        command_from_errors(8.0, 8.0, 10.0, true, 7360.0, 1240.0, 120.0, 68.0),
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
        let forced = forced_secondary_axis_command(
            &mut state, -180.0, 120.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0,
        )
        .map(|(direction, _)| direction);
        directions.push(forced);
    }

    assert_eq!(directions[0], None);
    assert_eq!(directions[1], Some(PtzDirection::Left));
    assert_eq!(directions[2], None);
    assert_eq!(directions[3], Some(PtzDirection::Left));

    let reset =
        forced_secondary_axis_command(&mut state, -80.0, 0.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0);
    assert_eq!(reset, None);
    let first_after_reset = forced_secondary_axis_command(
        &mut state, -400.0, 200.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0,
    );
    assert_eq!(first_after_reset, None);
}

#[test]
fn forced_secondary_axis_command_does_not_starve_pan_near_target() {
    let mut state = DualAxisInterleaveState::default();
    let mut forced_pan = 0usize;
    for _ in 0..8 {
        let forced = forced_secondary_axis_command(
            &mut state, 145.0, -125.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0,
        )
        .map(|(direction, _)| direction);
        if matches!(forced, Some(PtzDirection::Right)) {
            forced_pan = forced_pan.saturating_add(1);
        }
    }
    assert!(forced_pan >= 2);
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
fn calibrated_success_tolerance_respects_deadband_floor_and_cap() {
    assert_eq!(calibrated_success_tolerance(73.0, 240.0, 12.0), 120.0);
    assert_eq!(calibrated_success_tolerance(12.0, 62.0, 12.0), 68.0);
    assert_eq!(calibrated_success_tolerance(12.0, 0.0, 12.0), 12.0);
    assert_eq!(calibrated_success_tolerance(12.0, f64::NAN, 12.0), 12.0);
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
        1240.0,
        120.0,
        68.0
    ));
    assert!(!axis_swap_lag_detected(
        40.0,
        500.0,
        PtzDirection::Up,
        7360.0,
        1240.0,
        120.0,
        68.0
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
    assert!(success_latch_stagnation_ready(4, idle));
    assert!(!success_latch_stagnation_ready(3, idle));
    assert!(!success_latch_stagnation_ready(10, moving));

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
fn best_stagnation_near_miss_allows_single_axis_small_overrun_only() {
    let within_margin = super::BestObservedState {
        pan_count: 0,
        tilt_count: 0,
        pan_abs_error: 125,
        tilt_abs_error: 30,
    };
    assert!(best_stagnation_near_miss_eligible(
        within_margin,
        120.0,
        68.0,
        15.0
    ));

    let both_over = super::BestObservedState {
        pan_count: 0,
        tilt_count: 0,
        pan_abs_error: 130,
        tilt_abs_error: 90,
    };
    assert!(!best_stagnation_near_miss_eligible(
        both_over, 120.0, 68.0, 15.0
    ));

    let over_margin = super::BestObservedState {
        pan_count: 0,
        tilt_count: 0,
        pan_abs_error: 136,
        tilt_abs_error: 20,
    };
    assert!(!best_stagnation_near_miss_eligible(
        over_margin,
        120.0,
        68.0,
        15.0
    ));
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
fn enforce_residual_command_activity_reactivates_when_guard_suppresses_before_success() {
    let restored = enforce_residual_command_activity(4.8, 21.0, 12.0, 12.0);
    assert!(restored > 12.0);
}

#[test]
fn enforce_residual_command_activity_preserves_guarded_value_within_success_band() {
    assert_eq!(
        enforce_residual_command_activity(4.8, 11.0, 12.0, 12.0),
        4.8
    );
    assert_eq!(
        enforce_residual_command_activity(-3.2, -10.0, 12.0, 12.0),
        -3.2
    );
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
fn parse_onvif_duration_ms_parses_seconds_and_fraction() {
    assert_eq!(parse_onvif_duration_ms("PT1S"), Some(1_000));
    assert_eq!(parse_onvif_duration_ms("PT0.5S"), Some(500));
    assert_eq!(parse_onvif_duration_ms("PT0S"), Some(0));
    assert_eq!(parse_onvif_duration_ms("P1DT1S"), None);
}

#[test]
fn should_force_cgi_for_onvif_options_when_relative_move_unavailable_and_timeout_floor_is_high() {
    let options = OnvifPtzConfigurationOptions {
        supports_continuous_pan_tilt_velocity: true,
        supports_relative_pan_tilt_translation: false,
        supports_relative_pan_tilt_speed: true,
        has_timeout_range: true,
        timeout_min: Some("PT1S".to_string()),
        timeout_max: Some("PT10S".to_string()),
    };
    assert!(should_force_cgi_for_onvif_options(Some(&options)));

    let relative_supported = OnvifPtzConfigurationOptions {
        supports_relative_pan_tilt_translation: true,
        ..options.clone()
    };
    assert!(!should_force_cgi_for_onvif_options(Some(
        &relative_supported
    )));

    let short_timeout = OnvifPtzConfigurationOptions {
        timeout_min: Some("PT0S".to_string()),
        ..options
    };
    assert!(!should_force_cgi_for_onvif_options(Some(&short_timeout)));
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
    let pulse =
        pulse_ms_for_direction_with_lut(PtzDirection::Right, 90.0, 0.0, &pan_lut, &tilt_lut, 90.0);
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
        pan_lut.counts_per_ms(crate::app::usecases::ptz_pulse_lut::AxisDirection::Positive) > 1.0
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
fn near_target_speed1_pulse_ms_clamps_to_guard_band() {
    assert_eq!(
        near_target_speed1_pulse_ms(0, 120.0, PtzDirection::Right),
        0
    );
    assert_eq!(
        near_target_speed1_pulse_ms(24, 120.0, PtzDirection::Right),
        24
    );
    assert_eq!(
        near_target_speed1_pulse_ms(90, 120.0, PtzDirection::Right),
        45
    );
    assert_eq!(near_target_speed1_pulse_ms(24, 24.0, PtzDirection::Down), 0);
}

#[test]
fn update_reversal_counter_detects_near_target_sign_flips() {
    let mut counter = 0usize;
    let mut previous = None;
    update_reversal_counter(&mut counter, &mut previous, 140.0, 50.0, 120.0);
    update_reversal_counter(&mut counter, &mut previous, -130.0, 50.0, 120.0);
    assert_eq!(counter, 1);
    update_reversal_counter(&mut counter, &mut previous, 800.0, 50.0, 120.0);
    assert_eq!(counter, 0);
}

#[test]
fn oscillation_damping_active_only_near_success_band_and_after_reversals() {
    assert!(oscillation_damping_active(2, 1, 320.0, -180.0, 120.0, 68.0));
    assert!(!oscillation_damping_active(
        1, 1, 320.0, -180.0, 120.0, 68.0
    ));
    assert!(!oscillation_damping_active(
        3, 1, 500.0, -180.0, 120.0, 68.0
    ));
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
