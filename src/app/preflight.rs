use crate::core::command::CgiCommand;
use crate::core::error::AppResult;
use crate::core::model::DeviceInfo;
use crate::reolink::client::Client;
use crate::reolink::device::{get_ability, get_dev_info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub device_info: DeviceInfo,
    pub supported_commands: Vec<CgiCommand>,
}

pub fn run_preflight(client: &Client, user_name: &str) -> AppResult<PreflightReport> {
    let ability = get_ability(client, user_name)?;
    let device_info = get_dev_info(client)?;

    Ok(PreflightReport {
        device_info,
        supported_commands: ability.supported_commands,
    })
}
