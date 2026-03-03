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
mod tests;
