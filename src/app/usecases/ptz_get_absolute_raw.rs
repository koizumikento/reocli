use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzStatus;
use crate::reolink::client::Client;
use crate::reolink::ptz;

#[derive(Debug, Clone, PartialEq)]
pub struct PtzRawPosition {
    pub channel: u8,
    pub pan_count: i64,
    pub tilt_count: i64,
    pub zoom_count: Option<i64>,
    pub focus_count: Option<i64>,
}

pub fn execute(client: &Client, channel: u8) -> AppResult<PtzRawPosition> {
    let status = ptz::get_ptz_cur_pos(client, channel)?;
    map_status_to_raw_position(&status)
}

pub(crate) fn map_status_to_raw_position(status: &PtzStatus) -> AppResult<PtzRawPosition> {
    let pan_count = status.pan_position.ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "PTZ status missing pan count for channel {}",
                status.channel
            ),
        )
    })?;
    let tilt_count = status.tilt_position.ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "PTZ status missing tilt count for channel {}",
                status.channel
            ),
        )
    })?;

    Ok(PtzRawPosition {
        channel: status.channel,
        pan_count,
        tilt_count,
        zoom_count: status.zoom_position,
        focus_count: status.focus_position,
    })
}
