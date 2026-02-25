use crate::core::error::AppResult;
use crate::core::model::Snapshot;
use crate::reolink::client::Client;
use crate::reolink::media;

pub fn execute(client: &Client, channel: u8) -> AppResult<Snapshot> {
    media::snap(client, channel)
}
