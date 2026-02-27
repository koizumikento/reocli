use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzDirection;
use crate::interfaces::runtime::{self, OnvifConfig, PtzBackend};
use crate::reolink::client::Client;
use crate::reolink::{onvif, ptz};

const ONVIF_MOTION_SETTLE_MS: u64 = 180;
const FINE_RELATIVE_MIN_COUNT: i64 = 1;
const FINE_RELATIVE_MIN_DURATION_MS: u64 = 16;
const FINE_RELATIVE_MAX_DURATION_MS: u64 = 120;
const FINE_RELATIVE_DURATION_PER_COUNT_MS: f64 = 1.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportMotionHint {
    pub moving: Option<bool>,
    pub move_age_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct MotionTelemetry {
    last_move_started: Instant,
    expected_motion_until: Option<Instant>,
}

pub fn move_ptz(
    client: &Client,
    channel: u8,
    direction: PtzDirection,
    speed: u8,
    duration_ms: Option<u64>,
) -> AppResult<()> {
    match runtime::ptz_backend_from_env() {
        PtzBackend::Cgi => ptz::move_ptz(client, channel, direction, speed, duration_ms),
        PtzBackend::OnvifContinuous => {
            let started_at = Instant::now();
            let config = onvif_config()?;
            onvif::continuous_move(&config, channel, direction, speed, duration_ms)?;
            remember_motion(channel, started_at, duration_ms);
            Ok(())
        }
    }
}

pub fn stop_ptz(client: &Client, channel: u8) -> AppResult<()> {
    match runtime::ptz_backend_from_env() {
        PtzBackend::Cgi => ptz::stop_ptz(client, channel),
        PtzBackend::OnvifContinuous => {
            let config = onvif_config()?;
            onvif::stop(&config, channel)?;
            remember_motion(channel, Instant::now(), Some(0));
            Ok(())
        }
    }
}

pub fn supports_relative_move() -> bool {
    matches!(runtime::ptz_backend_from_env(), PtzBackend::OnvifContinuous)
}

pub fn supports_relative_move_for_channel(client: &Client, channel: u8) -> AppResult<bool> {
    let _ = client;
    let PtzBackend::OnvifContinuous = runtime::ptz_backend_from_env() else {
        return Ok(false);
    };

    let options = get_onvif_configuration_options(client, channel)?;
    Ok(options
        .as_ref()
        .map(|opts| opts.supports_relative_pan_tilt_translation)
        .unwrap_or(true))
}

pub fn move_relative_ptz(
    client: &Client,
    channel: u8,
    pan_delta_count: i64,
    tilt_delta_count: i64,
) -> AppResult<bool> {
    if !supports_relative_move_for_channel(client, channel)? {
        return Ok(false);
    }

    let dominant_error_count = pan_delta_count
        .saturating_abs()
        .max(tilt_delta_count.saturating_abs());
    if dominant_error_count == 0 {
        return Ok(true);
    }

    let speed = relative_speed_for_count(dominant_error_count);
    let started_at = Instant::now();
    let config = onvif_config()?;
    onvif::relative_move(
        &config,
        channel,
        pan_delta_count as f64,
        tilt_delta_count as f64,
        speed,
    )?;
    remember_motion(
        channel,
        started_at,
        Some(relative_duration_for_count(dominant_error_count)),
    );
    Ok(true)
}

pub fn get_onvif_status(client: &Client, channel: u8) -> AppResult<Option<onvif::OnvifPtzStatus>> {
    let _ = client;
    let PtzBackend::OnvifContinuous = runtime::ptz_backend_from_env() else {
        return Ok(None);
    };

    let config = onvif_config()?;
    onvif::get_status(&config, channel).map(Some)
}

pub fn get_onvif_configuration_options(
    client: &Client,
    channel: u8,
) -> AppResult<Option<onvif::OnvifPtzConfigurationOptions>> {
    let _ = client;
    let PtzBackend::OnvifContinuous = runtime::ptz_backend_from_env() else {
        return Ok(None);
    };

    let config = onvif_config()?;
    onvif::get_configuration_options(&config, channel).map(Some)
}

pub fn motion_status_hint(client: &Client, channel: u8) -> Option<TransportMotionHint> {
    let PtzBackend::OnvifContinuous = runtime::ptz_backend_from_env() else {
        return None;
    };

    let cached_hint = cached_motion_status_hint(channel);
    let onvif_hint = get_onvif_status(client, channel)
        .ok()
        .flatten()
        .and_then(|status| {
            map_onvif_move_status(status.pan_tilt_move_status, status.zoom_move_status)
        });

    match (onvif_hint, cached_hint) {
        (Some(onvif_moving), Some(cached)) => Some(TransportMotionHint {
            moving: combine_moving_hint(Some(onvif_moving), cached.moving),
            move_age_ms: cached.move_age_ms,
        }),
        (Some(onvif_moving), None) => Some(TransportMotionHint {
            moving: Some(onvif_moving),
            move_age_ms: None,
        }),
        (None, Some(cached)) => Some(cached),
        (None, None) => Some(TransportMotionHint {
            moving: None,
            move_age_ms: None,
        }),
    }
}

fn cached_motion_status_hint(channel: u8) -> Option<TransportMotionHint> {
    let telemetry = motion_telemetry_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&channel).copied());
    let now = Instant::now();
    telemetry.map(|entry| {
        let move_age_ms = Some(
            now.saturating_duration_since(entry.last_move_started)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        let moving = entry.expected_motion_until.map(|until| now < until);
        TransportMotionHint {
            moving,
            move_age_ms,
        }
    })
}

fn map_onvif_move_status(
    pan_tilt: Option<onvif::OnvifMoveStatus>,
    zoom: Option<onvif::OnvifMoveStatus>,
) -> Option<bool> {
    use onvif::OnvifMoveStatus::{Idle, Moving, Unknown};

    if matches!(pan_tilt, Some(Moving)) || matches!(zoom, Some(Moving)) {
        return Some(true);
    }
    if matches!(pan_tilt, Some(Idle)) || matches!(zoom, Some(Idle)) {
        return Some(false);
    }
    if matches!(pan_tilt, Some(Unknown)) || matches!(zoom, Some(Unknown)) {
        return None;
    }
    None
}

fn combine_moving_hint(primary: Option<bool>, secondary: Option<bool>) -> Option<bool> {
    match (primary, secondary) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        (Some(false), None) | (None, Some(false)) => Some(false),
        (None, None) => None,
    }
}

fn relative_speed_for_count(error_count: i64) -> u8 {
    if error_count <= 20 {
        1
    } else if error_count <= 45 {
        2
    } else if error_count <= 80 {
        3
    } else {
        4
    }
}

fn relative_duration_for_count(error_count: i64) -> u64 {
    let dominant = error_count.max(FINE_RELATIVE_MIN_COUNT) as f64;
    let duration = (dominant * FINE_RELATIVE_DURATION_PER_COUNT_MS).round() as u64;
    duration.clamp(FINE_RELATIVE_MIN_DURATION_MS, FINE_RELATIVE_MAX_DURATION_MS)
}

fn remember_motion(channel: u8, started_at: Instant, duration_ms: Option<u64>) {
    let expected_motion_until = duration_ms.and_then(|duration| {
        let bounded_duration = duration.saturating_add(ONVIF_MOTION_SETTLE_MS);
        started_at.checked_add(Duration::from_millis(bounded_duration))
    });

    if let Ok(mut cache) = motion_telemetry_cache().lock() {
        cache.insert(
            channel,
            MotionTelemetry {
                last_move_started: started_at,
                expected_motion_until,
            },
        );
    }
}

fn motion_telemetry_cache() -> &'static Mutex<HashMap<u8, MotionTelemetry>> {
    static CACHE: OnceLock<Mutex<HashMap<u8, MotionTelemetry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn onvif_config() -> AppResult<onvif::OnvifConfig> {
    let OnvifConfig {
        device_service_url,
        user_name,
        password,
        profile_token,
    } = runtime::onvif_config_from_env().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "ONVIF backend requires REOCLI_PASSWORD and a valid ONVIF device service URL"
                .to_string(),
        )
    })?;

    Ok(onvif::OnvifConfig::with_defaults(
        device_service_url,
        user_name,
        password,
        profile_token,
    ))
}
