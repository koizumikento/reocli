use crate::core::error::AppResult;
use crate::core::model::PtzStatus;
use crate::reolink::client::Client;
use crate::reolink::ptz;

pub fn execute(client: &Client, channel: u8) -> AppResult<PtzStatus> {
    ptz::get_ptz_status(client, channel)
}
