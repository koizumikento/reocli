use crate::core::error::AppResult;
use crate::core::model::PtzDirection;
use crate::reolink::client::Client;
use crate::reolink::ptz;

pub fn execute(
    client: &Client,
    channel: u8,
    direction: PtzDirection,
    speed: u8,
    duration_ms: Option<u64>,
) -> AppResult<()> {
    ptz::move_ptz(client, channel, direction, speed, duration_ms)
}
