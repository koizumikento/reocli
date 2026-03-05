use super::{
    AxisController, AxisControllerConfig, AxisEkf, AxisEkfConfig, EKF_NIS_HARD_GATE,
    quantize_normalized_u,
};
use crate::core::model::{AxisModelParams, AxisState};

fn full_trust_output_for_next_update(ekf: &AxisEkf, control_u: f64, measurement: f64) -> f64 {
    let u = control_u.clamp(-1.0, 1.0);
    let ts = ekf.config.ts_sec.max(1e-3);
    let predicted_state = AxisState {
        position: ekf.state.position + ts * ekf.state.velocity,
        velocity: ekf.model.alpha * ekf.state.velocity + ekf.model.beta * u,
        bias: ekf.state.bias,
    };
    let a = [[1.0, ts, 0.0], [0.0, ekf.model.alpha, 0.0], [0.0, 0.0, 1.0]];
    let q_scale = ekf
        .adaptive_q_scale
        .clamp(super::EKF_MIN_Q_SCALE, super::EKF_MAX_Q_SCALE);
    let p_pred = super::add_3x3(
        super::mul_3x3(super::mul_3x3(a, ekf.covariance), super::transpose_3x3(a)),
        [
            [ekf.config.q_position.max(1e-6) * q_scale, 0.0, 0.0],
            [0.0, ekf.config.q_velocity.max(1e-6) * q_scale, 0.0],
            [0.0, 0.0, ekf.config.q_bias.max(1e-8) * q_scale],
        ],
    );

    let innovation = measurement - super::output_from_state(predicted_state);
    let h = [1.0, 0.0, 1.0];
    let ph_t = super::mul_3x3_3x1(p_pred, h);
    let r = ekf
        .adaptive_r
        .clamp(super::EKF_MIN_MEASUREMENT_R, super::EKF_MAX_MEASUREMENT_R);
    let s = (super::dot_3(h, ph_t) + r.max(1e-6)).max(1e-6);
    let gain = super::scale_3(ph_t, 1.0 / s);
    let corrected_state = AxisState {
        position: (predicted_state.position + gain[0] * innovation)
            .clamp(ekf.config.min_position, ekf.config.max_position),
        velocity: (predicted_state.velocity + gain[1] * innovation)
            .clamp(ekf.config.min_velocity, ekf.config.max_velocity),
        bias: (predicted_state.bias + gain[2] * innovation)
            .clamp(ekf.config.min_bias, ekf.config.max_bias),
    };
    super::output_from_state(corrected_state)
}

#[test]
fn state_update_converges_toward_measurement() {
    let controller = AxisController::new(
        AxisControllerConfig {
            ts_sec: 0.05,
            min_position: -180.0,
            max_position: 180.0,
            stop_deadband_deg: 0.05,
        },
        AxisModelParams {
            alpha: 0.9,
            beta: 0.4,
        },
    );
    let measured_position = 10.0;
    let mut state = AxisState {
        position: 25.0,
        velocity: 0.0,
        bias: 0.0,
    };
    let initial_error = (state.position + state.bias - measured_position).abs();

    for _ in 0..12 {
        let (estimate, _) = controller.update(state, measured_position, measured_position);
        state = estimate.state;
    }

    let final_error = (state.position + state.bias - measured_position).abs();
    assert!(
        final_error < initial_error,
        "expected final error {final_error} < initial error {initial_error}"
    );
}

#[test]
fn out_of_range_target_is_clipped() {
    let controller = AxisController::new(
        AxisControllerConfig {
            ts_sec: 0.05,
            min_position: -30.0,
            max_position: 30.0,
            stop_deadband_deg: 0.01,
        },
        AxisModelParams {
            alpha: 0.85,
            beta: 0.5,
        },
    );
    let state = AxisState::default();
    let measured_position = 0.0;

    let (_, high_out_of_range_u) = controller.update(state, 1_000.0, measured_position);
    let (_, high_clipped_u) = controller.update(state, 30.0, measured_position);
    assert!((high_out_of_range_u - high_clipped_u).abs() < f64::EPSILON);

    let (_, low_out_of_range_u) = controller.update(state, -1_000.0, measured_position);
    let (_, low_clipped_u) = controller.update(state, -30.0, measured_position);
    assert!((low_out_of_range_u - low_clipped_u).abs() < f64::EPSILON);
}

#[test]
fn quantization_maps_deadband_and_speed_extremes() {
    assert_eq!(quantize_normalized_u(0.0, 0.1), None);

    let (_, small_speed) = quantize_normalized_u(0.11, 0.1).expect("should map to a move command");
    assert!(small_speed >= 1);

    assert_eq!(quantize_normalized_u(1.0, 0.0), Some((1, 64)));
    assert_eq!(quantize_normalized_u(-1.0, 0.0), Some((-1, 64)));
}

#[test]
fn ekf_tracks_measurement_and_estimates_velocity() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );

    let mut measurement = 0.0;
    for _ in 0..25 {
        measurement += 1.2;
        let _ = ekf.update(0.4, measurement);
    }

    let state = ekf.state();
    assert!(state.position > 15.0);
    assert!(state.velocity > 0.0);
    assert!((ekf.output() - measurement).abs() < 5.0);
}

#[test]
fn ekf_rejects_large_measurement_outlier() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );
    let _ = ekf.update(0.0, 2.0);
    let output_before = ekf.output();

    let outlier = 5_000.0;
    let _ = ekf.update(0.0, outlier);
    let output_after = ekf.output();
    let consistency = ekf.consistency();

    assert!(
        (output_after - output_before).abs() < 10.0,
        "outlier should be gated out, before={output_before}, after={output_after}"
    );
    assert!(
        (output_after - outlier).abs() > 1_000.0,
        "state should not be pulled close to the outlier, output={output_after}, outlier={outlier}"
    );
    assert!(
        consistency.last_nis >= EKF_NIS_HARD_GATE,
        "hard gate should still protect extreme outliers, nis={}",
        consistency.last_nis
    );
}

#[test]
fn ekf_normal_measurement_updates_state() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );
    let measurement = 5.0;
    let error_before = (ekf.output() - measurement).abs();
    let _ = ekf.update(0.0, measurement);
    let error_after = (ekf.output() - measurement).abs();

    assert!(error_after < error_before);
}

#[test]
fn ekf_inlier_update_matches_classic_kalman_step() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );
    let measurement = 2.0;
    let expected_output = full_trust_output_for_next_update(&ekf, 0.0, measurement);

    let _ = ekf.update(0.0, measurement);
    let output_after = ekf.output();

    assert!(
        (output_after - expected_output).abs() < 1e-9,
        "inlier update should match the non-robust EKF step, expected={expected_output}, actual={output_after}"
    );
}

#[test]
fn ekf_moderate_outlier_is_down_weighted_not_ignored() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );

    let output_before = ekf.output();
    let moderate_outlier = 8.0;
    let full_trust_output = full_trust_output_for_next_update(&ekf, 0.0, moderate_outlier);

    let _ = ekf.update(0.0, moderate_outlier);
    let output_after = ekf.output();

    assert!(
        output_after > output_before + 1.0,
        "moderate outlier should still influence state (not ignored), before={output_before}, after={output_after}"
    );
    assert!(
        output_after < full_trust_output - 0.5,
        "moderate outlier should be down-weighted versus full trust, full_trust={full_trust_output}, robust={output_after}"
    );
}

#[test]
fn ekf_outlier_rejection_keeps_consistency_metrics_finite() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );

    let _ = ekf.update(0.0, 1.0e308);
    let state = ekf.state();
    let consistency = ekf.consistency();

    assert!(state.position.is_finite());
    assert!(state.velocity.is_finite());
    assert!(state.bias.is_finite());
    assert!(consistency.last_nis.is_finite());
    assert!(consistency.ewma_nis.is_finite());
    assert!(consistency.adaptive_r.is_finite());
    assert!(consistency.residual_variance_proxy.is_finite());
    assert!((0.05..=30.0).contains(&consistency.adaptive_r));
    assert!((0.05..=120.0).contains(&consistency.residual_variance_proxy));
}

#[test]
fn ekf_consistency_reports_nis_and_residual_proxy() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );

    let _ = ekf.update(0.3, 8.0);
    let consistency = ekf.consistency();
    assert!(consistency.last_nis.is_finite());
    assert!(consistency.ewma_nis.is_finite());
    assert!(consistency.adaptive_r.is_finite());
    assert!(consistency.residual_variance_proxy.is_finite());
}

#[test]
fn ekf_measurement_noise_hint_is_bounded() {
    let mut ekf = AxisEkf::new(
        AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0),
        AxisModelParams {
            alpha: 0.92,
            beta: 0.35,
        },
        0.0,
    );
    let baseline = ekf.consistency().adaptive_r;
    ekf.apply_measurement_noise_hint(100.0);
    let inflated = ekf.consistency().adaptive_r;
    assert!(inflated > baseline);
    assert!(inflated <= 30.0);

    ekf.apply_measurement_noise_hint(0.0);
    let reduced = ekf.consistency().adaptive_r;
    assert!(reduced < inflated);
    assert!(reduced >= 0.05);
}

#[test]
fn ekf_snapshot_restores_consistency_metrics() {
    let config = AxisEkfConfig::with_default_noise(0.05, -180.0, 180.0);
    let model = AxisModelParams {
        alpha: 0.92,
        beta: 0.35,
    };
    let mut ekf = AxisEkf::new(config, model, 0.0);
    let _ = ekf.update(0.4, 10.0);
    ekf.apply_measurement_noise_hint(1.4);
    let before = ekf.consistency();
    let snapshot = ekf.snapshot();

    let restored = AxisEkf::from_snapshot(config, model, snapshot).expect("snapshot is valid");
    let after = restored.consistency();
    assert!((after.last_nis - before.last_nis).abs() < 1e-9);
    assert!((after.ewma_nis - before.ewma_nis).abs() < 1e-9);
    assert!((after.residual_variance_proxy - before.residual_variance_proxy).abs() < 1e-9);
    assert!((after.adaptive_r - before.adaptive_r).abs() < 1e-9);
}
