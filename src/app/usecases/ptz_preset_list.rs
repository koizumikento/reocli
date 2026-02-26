use crate::core::error::AppResult;
use crate::core::model::PtzPreset;
use crate::reolink::client::Client;
use crate::reolink::ptz;

pub fn execute(client: &Client, channel: u8) -> AppResult<Vec<PtzPreset>> {
    ptz::list_presets(client, channel)
}
