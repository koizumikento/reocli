use crate::app::usecases;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::reolink::client::{Auth, Client};

use super::tools::supported_tools;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRequest {
    pub tool: String,
    pub arguments: Vec<String>,
}

const DEFAULT_ENDPOINT: &str = "https://camera.local";

pub fn handle_request(request: McpRequest) -> AppResult<String> {
    let client = Client::new(endpoint_from_env(), Auth::Anonymous);

    match request.tool.as_str() {
        "mcp.list_tools" => {
            let names = supported_tools()
                .iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
                .join(",");
            Ok(names)
        }
        "reolink.get_ability" => {
            let user_name = request
                .arguments
                .first()
                .cloned()
                .unwrap_or_else(|| "admin".to_string());
            let ability = usecases::get_ability::execute(&client, &user_name)?;
            let commands = ability
                .supported_commands
                .iter()
                .map(|command| command.as_str())
                .collect::<Vec<_>>()
                .join(",");
            Ok(format!(
                "{{\"user\":\"{}\",\"commands\":\"{}\"}}",
                ability.user_name, commands
            ))
        }
        "reolink.get_dev_info" => {
            let info = usecases::get_dev_info::execute(&client)?;
            Ok(format!(
                "{{\"model\":\"{}\",\"firmware\":\"{}\"}}",
                info.model, info.firmware
            ))
        }
        "reolink.snap" => {
            let channel = parse_channel(&request.arguments)?;
            let snapshot = usecases::snap::execute(&client, channel)?;
            Ok(format!(
                "{{\"channel\":{},\"image_path\":\"{}\"}}",
                snapshot.channel, snapshot.image_path
            ))
        }
        _ => Err(AppError::new(
            ErrorKind::UnsupportedCommand,
            format!("unknown tool: {}", request.tool),
        )),
    }
}

fn parse_channel(arguments: &[String]) -> AppResult<u8> {
    match arguments.first() {
        Some(raw) => raw.parse::<u8>().map_err(|_| {
            AppError::new(
                ErrorKind::InvalidInput,
                "channel argument must be an integer between 0 and 255",
            )
        }),
        None => Ok(0),
    }
}

fn endpoint_from_env() -> String {
    std::env::var("REOCLI_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}
