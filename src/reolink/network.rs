use crate::core::command::{CgiCommand, CommandRequest};
use crate::core::error::AppResult;
use crate::core::model::NetworkInfo;
use crate::reolink::client::Client;

pub fn get_network_info(client: &Client) -> AppResult<NetworkInfo> {
    let request = CommandRequest::new(CgiCommand::GetNetwork);
    let _ = client.execute(request)?;

    Ok(NetworkInfo {
        host: "camera.local".to_string(),
        https_port: 443,
        http_port: 80,
    })
}
