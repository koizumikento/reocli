use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{
    AdaptiveTimeoutRttEstimator, AxisOnlineGainTracker, DualAxisInterleaveState, EdgePushContext,
    EdgePushLockoutState, FailureModeCounters, OnlineLearningState, adaptive_axis_tolerance,
    adaptive_timeout_budget, apply_fine_phase_feedforward, apply_pending_pulse_observation,
    apply_reversal_guard, apply_tilt_backlash_compensation, axis_count_bounds,
    axis_one_percent_threshold, axis_percent_threshold, axis_stale_detected,
    axis_swap_lag_detected, best_stagnation_near_miss_eligible, best_within_success_tolerance,
    calibrated_success_tolerance, clamp_pan_reversal_micro_control, clamp_tilt_edge_control,
    clamp_tilt_reversal_micro_control, command_activation_tolerance, command_from_errors,
    control_axis_direction, control_pulse_ms_for_error, control_step_ms_for_error,
    dominant_failure_mode_label, edge_saturation_detected, ekf_config,
    enforce_residual_command_activity, ensure_active_command_pulse_ms,
    forced_secondary_axis_command, format_failure_mode_counters, load_stored_ekf_state,
    max_failure_mode_counters, measurement_noise_hint_scale, model_mismatch_detected,
    near_target_speed1_pulse_ms, normalized_vector_error, oscillation_damping_active,
    parse_failure_mode_counters, parse_onvif_duration_ms, pending_pulse_observation_for_command,
    position_stable_threshold_count, pulse_ms_for_direction_with_lut, relative_delta_from_error,
    remaining_control_step_sleep_duration, required_stable_steps_for_oscillation,
    save_stored_ekf_state, secondary_axis_interleave_interval,
    select_command_with_edge_push_lockout, select_control_error,
    should_force_cgi_for_onvif_options, should_retry_after_timeout,
    stagnation_near_miss_latch_eligible, stale_status_detected, strict_axis_focus_command,
    strict_success_tolerances, success_latch_ready, success_latch_stagnation_ready,
    timeout_blocker_label, timeout_latch_eligible, timeout_retry_budget_ms,
    update_reversal_counter,
};
use crate::app::usecases::ptz_controller::AxisEkf;
use crate::app::usecases::ptz_pulse_lut::AxisPulseLut;
use crate::app::usecases::ptz_settle_gate::{
    CompletionGateCapabilities, completion_gate_allows_success,
};
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
    let dominant =
        command_from_errors(220.0, -100.0, 10.0, 10.0, true, 1000.0, 1000.0, 120.0, 68.0)
            .expect("command should be produced");
    assert_eq!(dominant.0, PtzDirection::Right);

    let tie_break_pan =
        command_from_errors(120.0, -68.0, 10.0, 10.0, true, 1000.0, 1000.0, 120.0, 68.0)
            .expect("command should be produced");
    assert_eq!(tie_break_pan.0, PtzDirection::Right);

    let tie_break_tilt =
        command_from_errors(120.0, -68.0, 10.0, 10.0, false, 1000.0, 1000.0, 120.0, 68.0)
            .expect("command should be produced");
    assert_eq!(tie_break_tilt.0, PtzDirection::Down);

    let single_axis =
        command_from_errors(0.0, -110.0, 10.0, 10.0, true, 1000.0, 1000.0, 120.0, 68.0)
            .expect("command should be produced");
    assert_eq!(single_axis.0, PtzDirection::Down);
    assert!(dominant.1 >= 1.0);
}

#[test]
fn command_from_errors_uses_success_tolerance_priority_for_asymmetric_span() {
    assert_eq!(
        command_from_errors(200.0, 100.0, 10.0, 10.0, true, 7360.0, 1240.0, 120.0, 68.0)
            .map(|cmd| cmd.0),
        Some(PtzDirection::Right)
    );
    assert_eq!(
        command_from_errors(8.0, 8.0, 10.0, 10.0, true, 7360.0, 1240.0, 120.0, 68.0),
        None
    );
}

#[test]
fn secondary_axis_interleave_interval_scales_with_ratio() {
    assert_eq!(secondary_axis_interleave_interval(0.30, 0.25), 1);
    assert_eq!(secondary_axis_interleave_interval(0.30, 0.18), 2);
    assert_eq!(secondary_axis_interleave_interval(0.30, 0.12), 3);
    assert_eq!(secondary_axis_interleave_interval(0.30, 0.08), 3);
    assert_eq!(secondary_axis_interleave_interval(0.30, 0.04), 3);
}

#[test]
fn forced_secondary_axis_command_alternates_for_balanced_dual_axis_error() {
    let mut state = DualAxisInterleaveState::default();
    let mut directions = Vec::new();
    for _ in 0..4 {
        let forced = forced_secondary_axis_command(
            &mut state, -180.0, 120.0, 12.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0, 50.0, 24.0,
        )
        .map(|(direction, _)| direction);
        directions.push(forced);
    }

    assert_eq!(directions[0], None);
    assert_eq!(directions[1], Some(PtzDirection::Left));
    assert_eq!(directions[2], None);
    assert_eq!(directions[3], Some(PtzDirection::Left));

    let reset = forced_secondary_axis_command(
        &mut state, -80.0, 0.0, 12.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0, 50.0, 24.0,
    );
    assert_eq!(reset, None);
    let first_after_reset = forced_secondary_axis_command(
        &mut state, -400.0, 200.0, 12.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0, 50.0, 24.0,
    );
    assert_eq!(first_after_reset, None);
}

#[test]
fn forced_secondary_axis_command_does_not_starve_pan_near_target() {
    let mut state = DualAxisInterleaveState::default();
    let mut forced_pan = 0usize;
    for _ in 0..8 {
        let forced = forced_secondary_axis_command(
            &mut state, 145.0, -125.0, 12.0, 12.0, 7_360.0, 1_240.0, 120.0, 68.0, 50.0, 24.0,
        )
        .map(|(direction, _)| direction);
        if matches!(forced, Some(PtzDirection::Right)) {
            forced_pan = forced_pan.saturating_add(1);
        }
    }
    assert!(forced_pan >= 2);
}

#[test]
fn strict_axis_focus_command_prefers_remaining_axis_when_other_is_within_strict() {
    assert_eq!(
        strict_axis_focus_command(40.0, 30.0, 50.0, 24.0).map(|(direction, _)| direction),
        Some(PtzDirection::Up)
    );
    assert_eq!(
        strict_axis_focus_command(-70.0, 20.0, 50.0, 24.0).map(|(direction, _)| direction),
        Some(PtzDirection::Left)
    );
    assert_eq!(strict_axis_focus_command(70.0, 30.0, 50.0, 24.0), None);
}

#[test]
fn forced_secondary_axis_command_uses_strict_threshold_in_endgame() {
    let mut state = DualAxisInterleaveState::default();
    let mut forced = None;
    for _ in 0..3 {
        forced = forced_secondary_axis_command(
            &mut state, 80.0, 30.0, 12.0, 12.0, 7_360.0, 1_240.0, 120.0, 60.0, 50.0, 24.0,
        )
        .map(|(direction, _)| direction);
    }
    assert_eq!(forced, Some(PtzDirection::Up));
}

#[test]
fn strict_success_tolerances_defaults_match_runtime_defaults() {
    let (pan, tilt) = strict_success_tolerances();
    assert_eq!(pan, 50.0);
    assert_eq!(tilt, 24.0);
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
fn two_percent_threshold_is_capped_per_axis_span() {
    assert_eq!(axis_percent_threshold(7360.0, 0.02), 147.0);
    assert_eq!(axis_percent_threshold(1240.0, 0.02), 24.0);
}

#[test]
fn calibrated_success_tolerance_respects_deadband_floor_and_cap() {
    assert_eq!(calibrated_success_tolerance(73.0, 240.0, 12.0), 73.0);
    assert_eq!(calibrated_success_tolerance(12.0, 62.0, 12.0), 60.0);
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
fn axis_stale_detected_tracks_per_axis_motion_stalls() {
    assert!(axis_stale_detected(0.6, Some(0.2)));
    assert!(!axis_stale_detected(0.04, Some(0.2)));
    assert!(!axis_stale_detected(0.6, Some(2.0)));
    assert!(!axis_stale_detected(0.6, None));
}

#[test]
fn measurement_noise_hint_scale_inflates_on_mismatch_and_stale() {
    let inflated = measurement_noise_hint_scale(true, true, 3, 120.0, 60.0);
    assert!(inflated > 1.0);
    let settled = measurement_noise_hint_scale(false, false, 0, 20.0, 60.0);
    assert!(settled < 1.0);
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
    assert!(success_latch_stagnation_ready(6, idle));
    assert!(!success_latch_stagnation_ready(5, idle));
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
    assert_eq!(
        timeout_blocker_label(true, false, true, true),
        "completion_gate"
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
        8.0
    ));

    let both_over = super::BestObservedState {
        pan_count: 0,
        tilt_count: 0,
        pan_abs_error: 130,
        tilt_abs_error: 90,
    };
    assert!(!best_stagnation_near_miss_eligible(
        both_over, 120.0, 68.0, 8.0
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
        8.0
    ));
}

#[test]
fn stagnation_near_miss_latch_requires_unknown_backend_hint_and_stagnation() {
    let near_miss_best = super::BestObservedState {
        pan_count: 0,
        tilt_count: 0,
        pan_abs_error: 54,
        tilt_abs_error: 20,
    };
    let unknown_hint = Some(crate::app::usecases::ptz_transport::TransportMotionHint {
        moving: None,
        move_age_ms: Some(180),
    });
    let known_stopped_hint = Some(crate::app::usecases::ptz_transport::TransportMotionHint {
        moving: Some(false),
        move_age_ms: Some(180),
    });
    let moving_hint = Some(crate::app::usecases::ptz_transport::TransportMotionHint {
        moving: Some(true),
        move_age_ms: Some(40),
    });

    assert!(stagnation_near_miss_latch_eligible(
        near_miss_best,
        50.0,
        24.0,
        true,
        None
    ));
    assert!(stagnation_near_miss_latch_eligible(
        near_miss_best,
        50.0,
        24.0,
        true,
        unknown_hint
    ));
    assert!(!stagnation_near_miss_latch_eligible(
        near_miss_best,
        50.0,
        24.0,
        true,
        known_stopped_hint
    ));
    assert!(!stagnation_near_miss_latch_eligible(
        near_miss_best,
        50.0,
        24.0,
        true,
        moving_hint
    ));
    assert!(!stagnation_near_miss_latch_eligible(
        near_miss_best,
        50.0,
        24.0,
        false,
        None
    ));
}

#[test]
fn timeout_latch_eligible_allows_conservative_near_miss_path() {
    let near_miss_best = super::BestObservedState {
        pan_count: 0,
        tilt_count: 0,
        pan_abs_error: 54,
        tilt_abs_error: 20,
    };
    let stagnation_ready = true;
    let near_miss =
        stagnation_near_miss_latch_eligible(near_miss_best, 50.0, 24.0, stagnation_ready, None);
    assert!(near_miss);

    assert!(timeout_latch_eligible(
        0,
        0.20,
        false,
        stagnation_ready,
        near_miss,
        None
    ));

    let known_stopped_hint = Some(crate::app::usecases::ptz_transport::TransportMotionHint {
        moving: Some(false),
        move_age_ms: Some(180),
    });
    let known_near_miss = stagnation_near_miss_latch_eligible(
        near_miss_best,
        50.0,
        24.0,
        stagnation_ready,
        known_stopped_hint,
    );
    assert!(!known_near_miss);
    assert!(!timeout_latch_eligible(
        0,
        0.20,
        false,
        stagnation_ready,
        known_near_miss,
        known_stopped_hint
    ));
}

#[test]
fn timeout_retry_budget_is_bounded_and_scaled() {
    assert_eq!(timeout_retry_budget_ms(9_000), 27_000);
    assert_eq!(timeout_retry_budget_ms(25_000), 36_000);
    assert_eq!(timeout_retry_budget_ms(60_000), 36_000);
}

#[test]
fn adaptive_timeout_budget_requires_multiple_samples_and_tracks_rttvar() {
    let mut estimator = AdaptiveTimeoutRttEstimator::default();
    estimator.observe(Duration::from_millis(50));
    let single_sample_budget = adaptive_timeout_budget(4_000, estimator);
    assert_eq!(single_sample_budget.applied_slack_ms, 0);
    assert_eq!(single_sample_budget.sample_count, 1);

    estimator.observe(Duration::from_millis(350));
    let budget = adaptive_timeout_budget(4_000, estimator);
    assert_eq!(budget.sample_count, 2);
    assert_eq!(budget.raw_slack_ms, 375);
    assert_eq!(budget.applied_slack_ms, 375);
    assert_eq!(budget.effective_timeout_ms, 4_375);
    assert!((budget.srtt_ms - 87.5).abs() < 1e-6);
    assert!((budget.rttvar_ms - 93.75).abs() < 1e-6);
}

#[test]
fn adaptive_timeout_budget_caps_variance_slack() {
    let mut estimator = AdaptiveTimeoutRttEstimator::default();
    estimator.observe(Duration::from_millis(50));
    estimator.observe(Duration::from_millis(2_000));
    let budget = adaptive_timeout_budget(2_000, estimator);
    assert!(budget.raw_slack_ms > budget.slack_cap_ms);
    assert_eq!(budget.applied_slack_ms, 1_000);
    assert_eq!(budget.effective_timeout_ms, 3_000);
}

#[test]
fn edge_push_lockout_blocks_repeated_risky_push_and_releases() {
    let edge_context = EdgePushContext {
        pan_measure: 7_355.0,
        pan_min_count: 0.0,
        pan_max_count: 7_360.0,
        pan_span: 7_360.0,
        pan_error_measured: 300.0,
        pan_success_tolerance: 120.0,
        tilt_measure: 620.0,
        tilt_min_count: 0.0,
        tilt_max_count: 1_240.0,
        tilt_error_measured: 0.0,
        tilt_success_tolerance: 68.0,
    };
    let mut lockout = EdgePushLockoutState::default();

    let first = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        0.0,
        10.0,
        10.0,
        edge_context,
    );
    assert_eq!(first, (Some((PtzDirection::Right, 300.0)), false));

    let second = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        0.0,
        10.0,
        10.0,
        edge_context,
    );
    assert_eq!(second, (None, true));

    let third = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        0.0,
        10.0,
        10.0,
        edge_context,
    );
    assert_eq!(third, (None, true));

    let fourth = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        0.0,
        10.0,
        10.0,
        edge_context,
    );
    assert_eq!(fourth, (None, true));

    let fifth = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        0.0,
        10.0,
        10.0,
        edge_context,
    );
    assert_eq!(fifth, (Some((PtzDirection::Right, 300.0)), false));
}

#[test]
fn edge_push_lockout_prefers_other_axis_when_available() {
    let edge_context = EdgePushContext {
        pan_measure: 7_355.0,
        pan_min_count: 0.0,
        pan_max_count: 7_360.0,
        pan_span: 7_360.0,
        pan_error_measured: 300.0,
        pan_success_tolerance: 120.0,
        tilt_measure: 620.0,
        tilt_min_count: 0.0,
        tilt_max_count: 1_240.0,
        tilt_error_measured: -180.0,
        tilt_success_tolerance: 68.0,
    };
    let mut lockout = EdgePushLockoutState::default();
    let _ = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        -180.0,
        10.0,
        10.0,
        edge_context,
    );

    let blocked_with_fallback = select_command_with_edge_push_lockout(
        &mut lockout,
        Some((PtzDirection::Right, 300.0)),
        300.0,
        -180.0,
        10.0,
        10.0,
        edge_context,
    );
    assert_eq!(
        blocked_with_fallback,
        (Some((PtzDirection::Down, 180.0)), true)
    );
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
    assert!(blocked_by_deadband < 0.0);
    assert!(blocked_by_deadband.abs() <= 14.0);
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
    assert!(!completion_gate_allows_success(
        Some(true),
        Some(250),
        CompletionGateCapabilities::from_hint(Some(true), Some(250)),
        120,
        2,
        2,
        4,
    ));
    assert!(!completion_gate_allows_success(
        Some(false),
        Some(70),
        CompletionGateCapabilities::from_hint(Some(false), Some(70)),
        120,
        2,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        Some(false),
        Some(260),
        CompletionGateCapabilities::from_hint(Some(false), Some(260)),
        120,
        2,
        2,
        4,
    ));
}

#[test]
fn completion_gate_partial_backend_hint_requires_more_stability() {
    assert!(!completion_gate_allows_success(
        Some(false),
        None,
        CompletionGateCapabilities::from_hint(Some(false), None),
        120,
        2,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        Some(false),
        None,
        CompletionGateCapabilities::from_hint(Some(false), None),
        120,
        4,
        2,
        4,
    ));
    assert!(!completion_gate_allows_success(
        None,
        Some(260),
        CompletionGateCapabilities::from_hint(None, Some(260)),
        120,
        2,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        None,
        Some(260),
        CompletionGateCapabilities::from_hint(None, Some(260)),
        120,
        4,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        None,
        None,
        CompletionGateCapabilities::from_hint(None, None),
        120,
        4,
        2,
        4,
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
fn ensure_active_command_pulse_ms_restores_zero_pulse_outside_strict_band() {
    assert_eq!(
        ensure_active_command_pulse_ms(0, PtzDirection::Down, 10.0, 40.0, 20.0, 20.0),
        8
    );
    assert_eq!(
        ensure_active_command_pulse_ms(0, PtzDirection::Right, 12.0, 40.0, 20.0, 20.0),
        0
    );
    assert_eq!(
        ensure_active_command_pulse_ms(18, PtzDirection::Down, 10.0, 40.0, 20.0, 20.0),
        18
    );
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
    assert_eq!(adaptive_axis_tolerance(50.0, 4, 200.0), 74.0);
    assert_eq!(adaptive_axis_tolerance(180.0, 8, 200.0), 204.0);
}

#[test]
fn command_activation_tolerance_clamps_to_strict_threshold() {
    assert_eq!(command_activation_tolerance(45.9, 20.0), 20.0);
    assert_eq!(command_activation_tolerance(12.0, 20.0), 12.0);
    assert_eq!(command_activation_tolerance(45.9, f64::NAN), 45.9);
}

#[test]
fn control_pulse_ms_uses_micro_pulses_near_target() {
    assert_eq!(control_pulse_ms_for_error(80.0), 0);
    assert_eq!(control_pulse_ms_for_error(150.0), 20);
    assert_eq!(control_pulse_ms_for_error(300.0), 35);
}

#[test]
fn control_step_ms_is_compact_for_closed_loop_updates() {
    assert_eq!(control_step_ms_for_error(80.0), 140);
    assert_eq!(control_step_ms_for_error(150.0), 130);
    assert_eq!(control_step_ms_for_error(300.0), 120);
    assert_eq!(control_step_ms_for_error(800.0), 110);
    assert_eq!(control_step_ms_for_error(2_000.0), 130);
}

#[test]
fn remaining_control_step_sleep_duration_returns_only_budget_left() {
    assert_eq!(
        remaining_control_step_sleep_duration(120, Duration::from_millis(35)),
        Duration::from_millis(85)
    );
    assert_eq!(
        remaining_control_step_sleep_duration(120, Duration::from_millis(120)),
        Duration::ZERO
    );
}

#[test]
fn remaining_control_step_sleep_duration_saturates_at_zero_when_over_budget() {
    assert_eq!(
        remaining_control_step_sleep_duration(120, Duration::from_millis(220)),
        Duration::ZERO
    );
}

#[test]
fn clamp_pan_reversal_micro_control_restricts_pan_after_reversal() {
    assert_eq!(
        clamp_pan_reversal_micro_control(PtzDirection::Right, 3, 45, true, 180.0),
        (1, 10)
    );
    assert_eq!(
        clamp_pan_reversal_micro_control(PtzDirection::Up, 3, 45, true, 180.0),
        (3, 45)
    );
    assert_eq!(
        clamp_pan_reversal_micro_control(PtzDirection::Left, 3, 45, true, 260.0),
        (3, 45)
    );
    assert_eq!(
        clamp_pan_reversal_micro_control(PtzDirection::Left, 3, 45, false, 180.0),
        (3, 45)
    );
}

#[test]
fn clamp_tilt_reversal_micro_control_restricts_tilt_after_reversal() {
    assert_eq!(
        clamp_tilt_reversal_micro_control(PtzDirection::Down, 3, 45, true, 80.0, 30.0),
        (1, 10)
    );
    assert_eq!(
        clamp_tilt_reversal_micro_control(PtzDirection::Right, 3, 45, true, 80.0, 30.0),
        (3, 45)
    );
    assert_eq!(
        clamp_tilt_reversal_micro_control(PtzDirection::Up, 3, 45, true, 260.0, 30.0),
        (3, 45)
    );
}

#[test]
fn apply_tilt_backlash_compensation_boosts_small_reversal_commands() {
    assert_eq!(
        apply_tilt_backlash_compensation(8.0, 32.0, 24.0, true),
        24.0
    );
    assert_eq!(
        apply_tilt_backlash_compensation(-8.0, -32.0, 24.0, true),
        -24.0
    );
    assert_eq!(
        apply_tilt_backlash_compensation(30.0, 32.0, 24.0, true),
        30.0
    );
    assert_eq!(
        apply_tilt_backlash_compensation(8.0, 32.0, 24.0, false),
        8.0
    );
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
    let mut pan_gain_tracker = AxisOnlineGainTracker::seeded(model.beta);
    let mut tilt_gain_tracker = AxisOnlineGainTracker::seeded(model.beta);
    pan_gain_tracker.observe(0.5, Some(90.0), 7_360.0);
    tilt_gain_tracker.observe(-0.5, Some(-70.0), 1_240.0);
    let mut pan_lut = AxisPulseLut::seeded(model.beta);
    let mut tilt_lut = AxisPulseLut::seeded(model.beta);
    pan_lut.update(
        crate::app::usecases::ptz_pulse_lut::AxisDirection::Positive,
        100,
        280.0,
    );
    tilt_lut.update(
        crate::app::usecases::ptz_pulse_lut::AxisDirection::Negative,
        120,
        180.0,
    );

    save_stored_ekf_state(
        &temp_file,
        state_key,
        channel,
        &pan_filter,
        &tilt_filter,
        OnlineLearningState {
            pan_gain_tracker: &pan_gain_tracker,
            tilt_gain_tracker: &tilt_gain_tracker,
            pan_pulse_lut: &pan_lut,
            tilt_pulse_lut: &tilt_lut,
            last_pan_u: 0.25,
            last_tilt_u: -0.5,
        },
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
    assert!(loaded.pan_positive_beta.is_some());
    assert!(loaded.pan_negative_beta.is_some());
    assert!(loaded.tilt_positive_beta.is_some());
    assert!(loaded.tilt_negative_beta.is_some());
    assert!(loaded.pan_positive_counts_per_ms.is_some());
    assert!(loaded.pan_negative_counts_per_ms.is_some());
    assert!(loaded.tilt_positive_counts_per_ms.is_some());
    assert!(loaded.tilt_negative_counts_per_ms.is_some());
    assert!(loaded.pan.last_nis.is_some());
    assert!(loaded.pan.ewma_nis.is_some());
    assert!(loaded.pan.residual_variance_proxy.is_some());
    assert!(loaded.tilt.last_nis.is_some());
    assert!(loaded.tilt.ewma_nis.is_some());
    assert!(loaded.tilt.residual_variance_proxy.is_some());

    let _ = fs::remove_file(PathBuf::from(&temp_file));
}

#[test]
fn legacy_ekf_state_loads_and_seeds_online_learning_params() {
    let temp_file = std::env::temp_dir().join(format!(
        "reocli-ekf-count-legacy-{}.json",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let channel = 1u8;
    let state_key = "camera-a.ch1";
    let legacy = serde_json::json!({
        "schema_version": 1,
        "state_key": state_key,
        "channel": channel,
        "updated_at": 123_u64,
        "last_pan_u": 0.1,
        "last_tilt_u": -0.2,
        "pan": {
            "position": 1000.0,
            "velocity": 10.0,
            "bias": 0.5,
            "covariance": [
                [4.0, 0.0, 0.0],
                [0.0, 16.0, 0.0],
                [0.0, 0.0, 4.0]
            ]
        },
        "tilt": {
            "position": -200.0,
            "velocity": -5.0,
            "bias": -0.3,
            "covariance": [
                [4.0, 0.0, 0.0],
                [0.0, 16.0, 0.0],
                [0.0, 0.0, 4.0]
            ]
        }
    });
    fs::write(
        &temp_file,
        serde_json::to_string_pretty(&legacy).expect("legacy state should serialize"),
    )
    .expect("legacy state file should be written");

    let loaded = load_stored_ekf_state(&temp_file, state_key, channel)
        .expect("legacy EKF state load should succeed")
        .expect("legacy EKF state should exist");
    assert!(loaded.pan_positive_beta.is_none());
    assert!(loaded.pan_negative_beta.is_none());
    assert!(loaded.pan_positive_counts_per_ms.is_none());
    assert!(loaded.pan_negative_counts_per_ms.is_none());

    let pan_tracker = AxisOnlineGainTracker::from_seed_and_betas(
        140.0,
        loaded.pan_positive_beta,
        loaded.pan_negative_beta,
    );
    assert!((pan_tracker.positive_beta() - 140.0).abs() < 1e-9);
    assert!((pan_tracker.negative_beta() - 140.0).abs() < 1e-9);

    let pan_lut = AxisPulseLut::from_seed_and_rates(
        120.0,
        loaded.pan_positive_counts_per_ms,
        loaded.pan_negative_counts_per_ms,
    );
    let positive =
        pan_lut.counts_per_ms(crate::app::usecases::ptz_pulse_lut::AxisDirection::Positive);
    let negative =
        pan_lut.counts_per_ms(crate::app::usecases::ptz_pulse_lut::AxisDirection::Negative);
    assert!((positive - 1.0).abs() < 1e-9);
    assert!((negative - 1.0).abs() < 1e-9);

    let _ = fs::remove_file(PathBuf::from(&temp_file));
}
