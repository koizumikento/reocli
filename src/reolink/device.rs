use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::AppResult;
use crate::core::model::{Ability, ChannelStatus, DeviceInfo};
use crate::reolink::client::Client;

pub fn get_ability(client: &Client, user_name: &str) -> AppResult<Ability> {
    let mut request = CommandRequest::new(CgiCommand::GetAbility);
    request.params = CommandParams {
        user_name: Some(user_name.to_string()),
        channel: None,
        payload: None,
    };

    let _ = client.execute(request)?;

    Ok(Ability {
        user_name: user_name.to_string(),
        supported_commands: vec![
            CgiCommand::GetAbility,
            CgiCommand::GetDevInfo,
            CgiCommand::Snap,
            CgiCommand::GetChannelStatus,
            CgiCommand::GetTime,
            CgiCommand::SetTime,
            CgiCommand::GetNetwork,
        ],
    })
}

pub fn get_dev_info(client: &Client) -> AppResult<DeviceInfo> {
    let request = CommandRequest::new(CgiCommand::GetDevInfo);
    let _ = client.execute(request)?;

    Ok(DeviceInfo {
        model: "Reolink-RLC-811A".to_string(),
        firmware: "v3.1.0".to_string(),
        serial_number: "RL-00000001".to_string(),
    })
}

pub fn get_channel_status(client: &Client, channel: u8) -> AppResult<ChannelStatus> {
    let mut request = CommandRequest::new(CgiCommand::GetChannelStatus);
    request.params = CommandParams {
        user_name: None,
        channel: Some(channel),
        payload: None,
    };

    let _ = client.execute(request)?;

    Ok(ChannelStatus {
        channel,
        online: true,
    })
}
