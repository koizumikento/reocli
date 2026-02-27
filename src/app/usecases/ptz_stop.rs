use crate::core::error::AppResult;
use crate::reolink::client::Client;

use super::ptz_transport;

pub fn execute(client: &Client, channel: u8) -> AppResult<()> {
    ptz_transport::stop_ptz(client, channel)
}
