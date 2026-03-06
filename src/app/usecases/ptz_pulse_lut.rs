#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisDirection {
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisPulseLut {
    positive_mid_counts_per_ms: f64,
    negative_mid_counts_per_ms: f64,
    positive_edge_counts_per_ms: f64,
    negative_edge_counts_per_ms: f64,
    ema_alpha: f64,
}

const COUNTS_PER_MS_MIN: f64 = 0.02;
const COUNTS_PER_MS_MAX: f64 = 14.0;
const COUNTS_PER_MS_FALLBACK: f64 = 0.4;
const EMA_ALPHA_DEFAULT: f64 = 0.3;
const MIN_VALID_OBSERVED_DELTA_COUNT: f64 = 1.0;

impl AxisPulseLut {
    pub fn seeded(model_beta: f64) -> Self {
        let seeded_rate = if model_beta.is_finite() && model_beta > 0.0 {
            // Model beta is roughly counts-per-step at normalized control.
            // Convert to counts-per-ms with a conservative effective step width.
            (model_beta / 120.0).clamp(COUNTS_PER_MS_MIN, COUNTS_PER_MS_MAX)
        } else {
            COUNTS_PER_MS_FALLBACK
        };
        Self {
            positive_mid_counts_per_ms: seeded_rate,
            negative_mid_counts_per_ms: seeded_rate,
            positive_edge_counts_per_ms: seeded_rate,
            negative_edge_counts_per_ms: seeded_rate,
            ema_alpha: EMA_ALPHA_DEFAULT,
        }
    }

    pub(crate) fn from_seed_and_rates(
        model_beta: f64,
        positive_mid_counts_per_ms: Option<f64>,
        negative_mid_counts_per_ms: Option<f64>,
        positive_edge_counts_per_ms: Option<f64>,
        negative_edge_counts_per_ms: Option<f64>,
    ) -> Self {
        let mut lut = Self::seeded(model_beta);
        if let Some(value) = sanitize_stored_rate(positive_mid_counts_per_ms) {
            lut.positive_mid_counts_per_ms = value;
        }
        if let Some(value) = sanitize_stored_rate(negative_mid_counts_per_ms) {
            lut.negative_mid_counts_per_ms = value;
        }
        if let Some(value) = sanitize_stored_rate(positive_edge_counts_per_ms) {
            lut.positive_edge_counts_per_ms = value;
        }
        if let Some(value) = sanitize_stored_rate(negative_edge_counts_per_ms) {
            lut.negative_edge_counts_per_ms = value;
        }
        lut
    }

    pub fn counts_per_ms(&self, direction: AxisDirection) -> f64 {
        self.counts_per_ms_in_band(direction, false)
    }

    pub fn counts_per_ms_in_band(&self, direction: AxisDirection, edge_band: bool) -> f64 {
        match (direction, edge_band) {
            (AxisDirection::Positive, false) => self.positive_mid_counts_per_ms,
            (AxisDirection::Negative, false) => self.negative_mid_counts_per_ms,
            (AxisDirection::Positive, true) => self.positive_edge_counts_per_ms,
            (AxisDirection::Negative, true) => self.negative_edge_counts_per_ms,
        }
    }

    pub fn update(&mut self, direction: AxisDirection, pulse_ms: u64, observed_delta_count: f64) {
        self.update_in_band(direction, false, pulse_ms, observed_delta_count);
    }

    pub fn update_in_band(
        &mut self,
        direction: AxisDirection,
        edge_band: bool,
        pulse_ms: u64,
        observed_delta_count: f64,
    ) {
        let sample_rate = sample_rate_from_observation(pulse_ms, observed_delta_count);
        let Some(sample_rate) = sample_rate else {
            return;
        };

        let previous = self.counts_per_ms_in_band(direction, edge_band);
        let alpha = self.ema_alpha.clamp(0.05, 0.95);
        let updated = ((1.0 - alpha) * previous) + (alpha * sample_rate);
        let updated = updated.clamp(COUNTS_PER_MS_MIN, COUNTS_PER_MS_MAX);
        match (direction, edge_band) {
            (AxisDirection::Positive, false) => self.positive_mid_counts_per_ms = updated,
            (AxisDirection::Negative, false) => self.negative_mid_counts_per_ms = updated,
            (AxisDirection::Positive, true) => self.positive_edge_counts_per_ms = updated,
            (AxisDirection::Negative, true) => self.negative_edge_counts_per_ms = updated,
        }
    }

    pub fn pulse_ms_for_target(
        &self,
        direction: AxisDirection,
        target_delta_count: f64,
        min_ms: u64,
        max_ms: u64,
    ) -> u64 {
        self.pulse_ms_for_target_in_band(direction, false, target_delta_count, min_ms, max_ms)
    }

    pub fn pulse_ms_for_target_in_band(
        &self,
        direction: AxisDirection,
        edge_band: bool,
        target_delta_count: f64,
        min_ms: u64,
        max_ms: u64,
    ) -> u64 {
        let (lower, upper) = normalize_pulse_bounds(min_ms, max_ms);
        let target = target_delta_count.abs();
        if !target.is_finite() || target <= f64::EPSILON {
            return lower;
        }

        let rate = self
            .counts_per_ms_in_band(direction, edge_band)
            .clamp(COUNTS_PER_MS_MIN, COUNTS_PER_MS_MAX);
        let raw_ms = (target / rate).round();
        if !raw_ms.is_finite() {
            return upper;
        }

        let raw_ms = raw_ms.max(lower as f64).min(upper as f64);
        raw_ms as u64
    }
}

fn sample_rate_from_observation(pulse_ms: u64, observed_delta_count: f64) -> Option<f64> {
    if pulse_ms == 0 || !observed_delta_count.is_finite() {
        return None;
    }

    let observed = observed_delta_count.abs();
    if observed < MIN_VALID_OBSERVED_DELTA_COUNT {
        return None;
    }

    let pulse_ms = pulse_ms as f64;
    if pulse_ms <= 0.0 || !pulse_ms.is_finite() {
        return None;
    }
    let sample_rate = observed / pulse_ms;
    if !sample_rate.is_finite() {
        return None;
    }
    Some(sample_rate.clamp(COUNTS_PER_MS_MIN, COUNTS_PER_MS_MAX))
}

fn sanitize_stored_rate(value: Option<f64>) -> Option<f64> {
    let value = value?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(COUNTS_PER_MS_MIN, COUNTS_PER_MS_MAX))
}

fn normalize_pulse_bounds(min_ms: u64, max_ms: u64) -> (u64, u64) {
    if min_ms <= max_ms {
        (min_ms, max_ms)
    } else {
        (max_ms, min_ms)
    }
}

#[cfg(test)]
mod tests;
