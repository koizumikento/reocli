use super::{PositionBand, classify_position_band, scale_directional_deadband};

#[test]
fn classify_position_band_splits_low_mid_high() {
    assert_eq!(classify_position_band(5.0, 0.0, 100.0), PositionBand::Low);
    assert_eq!(classify_position_band(50.0, 0.0, 100.0), PositionBand::Mid);
    assert_eq!(classify_position_band(95.0, 0.0, 100.0), PositionBand::High);
}

#[test]
fn classify_position_band_handles_reversed_or_invalid_bounds() {
    assert_eq!(classify_position_band(5.0, 100.0, 0.0), PositionBand::Low);
    assert_eq!(
        classify_position_band(f64::NAN, 0.0, 100.0),
        PositionBand::Mid
    );
    assert_eq!(classify_position_band(10.0, 10.0, 10.0), PositionBand::Mid);
}

#[test]
fn scale_directional_deadband_boosts_edges_only() {
    let center = scale_directional_deadband(100.0, 50.0, 0.0, 100.0);
    let edge = scale_directional_deadband(100.0, 95.0, 0.0, 100.0);
    assert_eq!(center, 100.0);
    assert!((edge - 118.0).abs() < 1e-6);
}

#[test]
fn scale_directional_deadband_returns_zero_for_non_positive_input() {
    assert_eq!(scale_directional_deadband(-1.0, 50.0, 0.0, 100.0), 0.0);
    assert_eq!(scale_directional_deadband(0.0, 50.0, 0.0, 100.0), 0.0);
}
