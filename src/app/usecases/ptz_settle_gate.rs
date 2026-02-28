#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PositionSettlingTracker {
    stable_steps: usize,
}

impl PositionSettlingTracker {
    pub fn new() -> Self {
        Self { stable_steps: 0 }
    }

    pub fn observe(
        &mut self,
        pan_delta: Option<f64>,
        tilt_delta: Option<f64>,
        threshold_count: f64,
    ) {
        let threshold = sanitize_threshold(threshold_count);
        let pan_stable = axis_delta_is_stable(pan_delta, threshold);
        let tilt_stable = axis_delta_is_stable(tilt_delta, threshold);

        if pan_stable && tilt_stable {
            self.stable_steps = self.stable_steps.saturating_add(1);
        } else {
            self.stable_steps = 0;
        }
    }

    pub fn stable_steps(self) -> usize {
        self.stable_steps
    }

    pub fn reset(&mut self) {
        self.stable_steps = 0;
    }
}

pub fn completion_gate_allows_success(
    moving: Option<bool>,
    move_age_ms: Option<u64>,
    min_age_ms: u64,
    stable_steps: usize,
    required_stable_steps: usize,
) -> bool {
    let required = required_stable_steps.max(1);
    if stable_steps < required {
        return false;
    }

    match moving {
        Some(true) => false,
        Some(false) => move_age_ms.is_none_or(|age_ms| age_ms >= min_age_ms),
        None => move_age_ms.is_some_and(|age_ms| age_ms >= min_age_ms),
    }
}

fn sanitize_threshold(raw: f64) -> f64 {
    if !raw.is_finite() {
        return 0.0;
    }
    raw.abs().max(0.0)
}

fn axis_delta_is_stable(delta: Option<f64>, threshold: f64) -> bool {
    let Some(delta) = delta else {
        return false;
    };
    if !delta.is_finite() {
        return false;
    }
    delta.abs() <= threshold
}

#[cfg(test)]
mod tests {
    use super::{PositionSettlingTracker, completion_gate_allows_success};

    #[test]
    fn tracker_accumulates_when_both_axes_are_within_threshold() {
        let mut tracker = PositionSettlingTracker::new();

        tracker.observe(Some(1.0), Some(-0.8), 2.0);
        tracker.observe(Some(0.5), Some(0.4), 2.0);

        assert_eq!(tracker.stable_steps(), 2);
    }

    #[test]
    fn tracker_resets_when_any_axis_exceeds_threshold() {
        let mut tracker = PositionSettlingTracker::new();

        tracker.observe(Some(0.5), Some(0.2), 1.0);
        tracker.observe(Some(1.5), Some(0.2), 1.0);

        assert_eq!(tracker.stable_steps(), 0);
    }

    #[test]
    fn tracker_resets_on_none_or_nan_inputs() {
        let mut tracker = PositionSettlingTracker::new();

        tracker.observe(Some(0.2), Some(0.3), 1.0);
        tracker.observe(None, Some(0.1), 1.0);
        assert_eq!(tracker.stable_steps(), 0);

        tracker.observe(Some(0.2), Some(0.3), 1.0);
        tracker.observe(Some(f64::NAN), Some(0.1), 1.0);
        assert_eq!(tracker.stable_steps(), 0);
    }

    #[test]
    fn completion_gate_blocks_while_moving_or_unstable() {
        assert!(!completion_gate_allows_success(
            Some(true),
            Some(300),
            120,
            3,
            2,
        ));
        assert!(!completion_gate_allows_success(
            Some(false),
            Some(300),
            120,
            1,
            2,
        ));
    }

    #[test]
    fn completion_gate_allows_stopped_with_sufficient_age() {
        assert!(completion_gate_allows_success(
            Some(false),
            Some(250),
            120,
            2,
            2,
        ));
    }

    #[test]
    fn completion_gate_allows_stopped_without_age_when_stable() {
        assert!(completion_gate_allows_success(Some(false), None, 120, 2, 2));
    }

    #[test]
    fn completion_gate_requires_age_when_moving_state_unknown() {
        assert!(!completion_gate_allows_success(None, None, 120, 3, 2));
        assert!(!completion_gate_allows_success(None, Some(80), 120, 3, 2));
        assert!(completion_gate_allows_success(None, Some(180), 120, 3, 2));
    }
}
