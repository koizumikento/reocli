#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionBand {
    Low,
    Mid,
    High,
}

const EDGE_BAND_RATIO: f64 = 0.2;
const EDGE_DEADBAND_MULTIPLIER: f64 = 1.18;
const MID_DEADBAND_MULTIPLIER: f64 = 1.0;

pub fn classify_position_band(position: f64, min_count: f64, max_count: f64) -> PositionBand {
    if !position.is_finite() || !min_count.is_finite() || !max_count.is_finite() {
        return PositionBand::Mid;
    }

    let low = min_count.min(max_count);
    let high = min_count.max(max_count);
    let span = high - low;
    if span <= f64::EPSILON {
        return PositionBand::Mid;
    }

    let lower_edge = low + (span * EDGE_BAND_RATIO);
    let upper_edge = high - (span * EDGE_BAND_RATIO);
    if position <= lower_edge {
        PositionBand::Low
    } else if position >= upper_edge {
        PositionBand::High
    } else {
        PositionBand::Mid
    }
}

pub fn scale_directional_deadband(
    base_deadband: f64,
    position: f64,
    min_count: f64,
    max_count: f64,
) -> f64 {
    if !base_deadband.is_finite() || base_deadband <= 0.0 {
        return 0.0;
    }

    let multiplier = match classify_position_band(position, min_count, max_count) {
        PositionBand::Low | PositionBand::High => EDGE_DEADBAND_MULTIPLIER,
        PositionBand::Mid => MID_DEADBAND_MULTIPLIER,
    };
    (base_deadband * multiplier).max(0.0)
}

#[cfg(test)]
mod tests {
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
}
