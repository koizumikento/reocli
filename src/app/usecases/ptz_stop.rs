use crate::core::error::AppResult;
use crate::reolink::client::Client;
use crate::reolink::ptz;

pub fn execute(client: &Client, channel: u8) -> AppResult<()> {
    ptz::stop_ptz(client, channel)
}
