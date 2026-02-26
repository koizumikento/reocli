use crate::core::error::AppResult;
use crate::core::model::Snapshot;
use crate::reolink::client::Client;
use crate::reolink::media;

pub fn execute(client: &Client, channel: u8) -> AppResult<Snapshot> {
    execute_with_out_path(client, channel, None)
}

pub fn execute_with_out_path(
    client: &Client,
    channel: u8,
    out_path: Option<&str>,
) -> AppResult<Snapshot> {
    media::snap_with_out_path(client, channel, out_path)
}
