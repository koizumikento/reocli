use super::*;
use crate::reolink::onvif::OnvifMoveStatus::{Idle, Moving, Unknown};

#[test]
fn map_onvif_move_status_prioritizes_moving_over_other_states() {
    assert_eq!(map_onvif_move_status(Some(Moving), Some(Idle)), Some(true));
    assert_eq!(
        map_onvif_move_status(Some(Unknown), Some(Moving)),
        Some(true)
    );
}

#[test]
fn map_onvif_move_status_maps_idle_when_no_moving_exists() {
    assert_eq!(
        map_onvif_move_status(Some(Unknown), Some(Idle)),
        Some(false)
    );
    assert_eq!(map_onvif_move_status(Some(Idle), None), Some(false));
}

#[test]
fn map_onvif_move_status_returns_none_for_unknown_or_absent_only() {
    assert_eq!(map_onvif_move_status(Some(Unknown), None), None);
    assert_eq!(map_onvif_move_status(None, Some(Unknown)), None);
    assert_eq!(map_onvif_move_status(None, None), None);
}

#[test]
fn combine_moving_hint_truth_table() {
    let cases = [
        ((Some(true), Some(true)), Some(true)),
        ((Some(true), Some(false)), Some(true)),
        ((Some(true), None), Some(true)),
        ((Some(false), Some(true)), Some(true)),
        ((Some(false), Some(false)), Some(false)),
        ((Some(false), None), Some(false)),
        ((None, Some(true)), Some(true)),
        ((None, Some(false)), Some(false)),
        ((None, None), None),
    ];

    for ((primary, secondary), expected) in cases {
        assert_eq!(combine_moving_hint(primary, secondary), expected);
    }
}

#[test]
fn relative_speed_for_count_uses_threshold_boundaries() {
    let cases = [(0, 1), (20, 1), (21, 2), (45, 2), (46, 3), (80, 3), (81, 4)];

    for (error_count, expected_speed) in cases {
        assert_eq!(relative_speed_for_count(error_count), expected_speed);
    }
}

#[test]
fn relative_duration_for_count_clamps_to_min_and_max() {
    assert_eq!(
        relative_duration_for_count(0),
        FINE_RELATIVE_MIN_DURATION_MS
    );
    assert_eq!(
        relative_duration_for_count(i64::MAX),
        FINE_RELATIVE_MAX_DURATION_MS
    );
}
