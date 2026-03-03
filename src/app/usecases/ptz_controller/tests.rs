use super::{AxisController, AxisControllerConfig, AxisEkf, AxisEkfConfig, quantize_normalized_u};
use crate::core::model::{AxisModelParams, AxisState};

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
