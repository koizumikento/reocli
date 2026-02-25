use crate::core::error::AppResult;
use crate::core::model::ChannelStatus;
use crate::reolink::client::Client;
use crate::reolink::device;

pub fn execute(client: &Client, channel: u8) -> AppResult<ChannelStatus> {
    device::get_channel_status(client, channel)
}
