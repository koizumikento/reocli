use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzDirection;
use crate::interfaces::runtime::{self, OnvifConfig, PtzBackend};
use crate::reolink::client::Client;
use crate::reolink::{onvif, ptz};

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
            let config = onvif_config()?;
            onvif::continuous_move(&config, channel, direction, speed, duration_ms)
        }
    }
}

pub fn stop_ptz(client: &Client, channel: u8) -> AppResult<()> {
    match runtime::ptz_backend_from_env() {
        PtzBackend::Cgi => ptz::stop_ptz(client, channel),
        PtzBackend::OnvifContinuous => {
            let config = onvif_config()?;
            onvif::stop(&config, channel)
        }
    }
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
