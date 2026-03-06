use super::{AxisDirection, AxisPulseLut};

#[test]
fn seeded_uses_model_beta_when_finite() {
    let lut = AxisPulseLut::seeded(240.0);
    let rate = lut.counts_per_ms(AxisDirection::Positive);
    assert!((rate - 2.0).abs() < 1e-6);
    assert_eq!(
        rate,
        lut.counts_per_ms(AxisDirection::Negative),
        "seed should be symmetric"
    );
}

#[test]
fn seeded_falls_back_for_invalid_beta() {
    let lut = AxisPulseLut::seeded(f64::NAN);
    let rate = lut.counts_per_ms(AxisDirection::Positive);
    assert!((rate - 0.4).abs() < 1e-6);
}

#[test]
fn update_applies_ema_for_valid_sample_only() {
    let mut lut = AxisPulseLut::seeded(120.0); // 1.0 counts/ms
    lut.update(AxisDirection::Positive, 100, 400.0); // 4.0 counts/ms

    let updated = lut.counts_per_ms(AxisDirection::Positive);
    // 0.7*1.0 + 0.3*4.0 = 1.9
    assert!((updated - 1.9).abs() < 1e-6);
    assert!(
        (lut.counts_per_ms(AxisDirection::Negative) - 1.0).abs() < 1e-6,
        "other direction should be unchanged"
    );
}

#[test]
fn update_in_band_only_changes_selected_band() {
    let mut lut = AxisPulseLut::seeded(120.0);
    let mid_base = lut.counts_per_ms_in_band(AxisDirection::Positive, false);
    let edge_base = lut.counts_per_ms_in_band(AxisDirection::Positive, true);

    lut.update_in_band(AxisDirection::Positive, true, 100, 500.0);

    assert!((lut.counts_per_ms_in_band(AxisDirection::Positive, false) - mid_base).abs() < 1e-6);
    assert!(lut.counts_per_ms_in_band(AxisDirection::Positive, true) > edge_base);
}

#[test]
fn update_ignores_noise_and_invalid_values() {
    let mut lut = AxisPulseLut::seeded(120.0);
    let base = lut.counts_per_ms(AxisDirection::Positive);

    lut.update(AxisDirection::Positive, 0, 20.0);
    lut.update(AxisDirection::Positive, 100, 0.2);
    lut.update(AxisDirection::Positive, 100, f64::NAN);
    assert!((lut.counts_per_ms(AxisDirection::Positive) - base).abs() < 1e-6);
}

#[test]
fn pulse_ms_for_target_uses_directional_rate_and_clamps() {
    let mut lut = AxisPulseLut::seeded(120.0);
    lut.update(AxisDirection::Positive, 100, 600.0); // rate ~= 2.5

    let pulse = lut.pulse_ms_for_target(AxisDirection::Positive, 100.0, 10, 120);
    assert_eq!(pulse, 40);

    let low = lut.pulse_ms_for_target(AxisDirection::Positive, 0.1, 10, 120);
    assert_eq!(low, 10);
    let high = lut.pulse_ms_for_target(AxisDirection::Positive, 9_999.0, 10, 120);
    assert_eq!(high, 120);
}

#[test]
fn pulse_ms_for_target_in_band_uses_band_specific_rate() {
    let lut = AxisPulseLut::from_seed_and_rates(120.0, Some(1.0), Some(1.0), Some(4.0), Some(1.0));

    let mid_pulse = lut.pulse_ms_for_target_in_band(AxisDirection::Positive, false, 80.0, 10, 120);
    let edge_pulse = lut.pulse_ms_for_target_in_band(AxisDirection::Positive, true, 80.0, 10, 120);

    assert_eq!(mid_pulse, 80);
    assert_eq!(edge_pulse, 20);
}
