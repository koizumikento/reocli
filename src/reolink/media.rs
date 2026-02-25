use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::AppResult;
use crate::core::model::Snapshot;
use crate::reolink::client::Client;

pub fn snap(client: &Client, channel: u8) -> AppResult<Snapshot> {
    let mut request = CommandRequest::new(CgiCommand::Snap);
    request.params = CommandParams {
        user_name: None,
        channel: Some(channel),
        payload: None,
    };

    let _ = client.execute(request)?;

    Ok(Snapshot {
        channel,
        image_path: format!("snapshots/channel-{channel}.jpg"),
    })
}
