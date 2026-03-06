use super::{CompletionGateCapabilities, PositionSettlingTracker, completion_gate_allows_success};

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
        CompletionGateCapabilities::from_hint(Some(true), Some(300)),
        120,
        3,
        2,
        4,
    ));
    assert!(!completion_gate_allows_success(
        Some(false),
        Some(300),
        CompletionGateCapabilities::from_hint(Some(false), Some(300)),
        120,
        1,
        2,
        4,
    ));
}

#[test]
fn completion_gate_full_hint_path_matches_previous_behavior() {
    assert!(!completion_gate_allows_success(
        Some(false),
        Some(80),
        CompletionGateCapabilities::from_hint(Some(false), Some(80)),
        120,
        2,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        Some(false),
        Some(250),
        CompletionGateCapabilities::from_hint(Some(false), Some(250)),
        120,
        2,
        2,
        4,
    ));
}

#[test]
fn completion_gate_partial_hint_falls_back_to_stricter_stability() {
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
        Some(180),
        CompletionGateCapabilities::from_hint(None, Some(180)),
        120,
        2,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        None,
        Some(180),
        CompletionGateCapabilities::from_hint(None, Some(180)),
        120,
        4,
        2,
        4,
    ));
}

#[test]
fn completion_gate_partial_hint_respects_age_when_present() {
    assert!(!completion_gate_allows_success(
        None,
        Some(80),
        CompletionGateCapabilities::from_hint(None, Some(80)),
        120,
        5,
        2,
        4,
    ));
    assert!(completion_gate_allows_success(
        None,
        None,
        CompletionGateCapabilities::from_hint(None, None),
        120,
        5,
        2,
        4,
    ));
}
