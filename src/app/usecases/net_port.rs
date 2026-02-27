use crate::core::error::AppResult;
use crate::core::model::NetPortSettings;
use crate::reolink::client::Client;
use crate::reolink::network;

pub fn get(client: &Client) -> AppResult<NetPortSettings> {
    network::get_net_port(client)
}

pub fn set_onvif_enabled(
    client: &Client,
    enabled: bool,
    onvif_port: Option<u16>,
) -> AppResult<NetPortSettings> {
    network::set_onvif_enabled(client, enabled, onvif_port)
}
