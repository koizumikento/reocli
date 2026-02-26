use crate::core::error::AppResult;
use crate::reolink::client::Client;
use crate::reolink::ptz;

pub fn execute(client: &Client, channel: u8, preset_id: u8) -> AppResult<()> {
    ptz::goto_preset(client, channel, preset_id)
}
