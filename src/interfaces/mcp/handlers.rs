use crate::app::usecases;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::reolink::client::{Auth, Client};
use serde_json::{Value, json};

use super::tools::supported_tools;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRequest {
    pub tool: String,
    pub arguments: Vec<String>,
}

const DEFAULT_ENDPOINT: &str = "https://camera.local";

pub fn handle_request(request: McpRequest) -> AppResult<String> {
    let (primary_auth, fallback_auth) = auth_from_env();
    let client = build_client(primary_auth, fallback_auth);

    match request.tool.as_str() {
        "mcp.list_tools" => {
            let names = supported_tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>();
            json_response(json!({ "tools": names }))
        }
        "reolink.get_user_auth" => {
            let (user_name, password) = parse_user_password(&request.arguments)?;
            let token = usecases::get_user_auth::execute(&client, &user_name, &password)?;
            json_response(json!({ "token": token }))
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
            json_response(json!({
                "user": ability.user_name,
                "commands": commands
            }))
        }
        "reolink.get_dev_info" => {
            let info = usecases::get_dev_info::execute(&client)?;
            json_response(json!({
                "model": info.model,
                "firmware": info.firmware
            }))
        }
        "reolink.get_channel_status" => {
            let channel = parse_channel(&request.arguments)?;
            let status = usecases::get_channel_status::execute(&client, channel)?;
            json_response(json!({
                "channel": status.channel,
                "online": status.online
            }))
        }
        "reolink.get_time" => {
            let time = usecases::get_time::execute(&client)?;
            json_response(json!({ "time": time.iso8601 }))
        }
        "reolink.set_time" => {
            let iso8601 = parse_iso8601(&request.arguments)?;
            let updated = usecases::set_time::execute(&client, &iso8601)?;
            json_response(json!({ "time": updated.iso8601 }))
        }
        "reolink.snap" => {
            let channel = parse_channel(&request.arguments)?;
            let snapshot = usecases::snap::execute(&client, channel)?;
            json_response(json!({
                "channel": snapshot.channel,
                "image_path": snapshot.image_path
            }))
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

fn parse_user_password(arguments: &[String]) -> AppResult<(String, String)> {
    let user_name = arguments.first().cloned().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.get_user_auth requires [user, password]",
        )
    })?;
    let password = arguments.get(1).cloned().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.get_user_auth requires [user, password]",
        )
    })?;
    Ok((user_name, password))
}

fn parse_iso8601(arguments: &[String]) -> AppResult<String> {
    arguments.first().cloned().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.set_time requires [iso8601]",
        )
    })
}

fn json_response(value: Value) -> AppResult<String> {
    serde_json::to_string(&value).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to serialize MCP response JSON: {error}"),
        )
    })
}

fn endpoint_from_env() -> String {
    std::env::var("REOCLI_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

fn build_client(primary_auth: Auth, fallback_auth: Option<Auth>) -> Client {
    let client = Client::new(endpoint_from_env(), primary_auth);
    match fallback_auth {
        Some(auth) => client.with_fallback_auth(auth),
        None => client,
    }
}

fn auth_from_env() -> (Auth, Option<Auth>) {
    let fallback_auth = match (
        std::env::var("REOCLI_USER"),
        std::env::var("REOCLI_PASSWORD"),
    ) {
        (Ok(user), Ok(password)) if !user.trim().is_empty() && !password.is_empty() => {
            Some(Auth::UserPassword { user, password })
        }
        _ => None,
    };

    if let Ok(token) = std::env::var("REOCLI_TOKEN") {
        if !token.trim().is_empty() {
            return (Auth::Token(token), fallback_auth);
        }
    }

    match fallback_auth {
        Some(user_password_auth) => (user_password_auth, None),
        None => (Auth::Anonymous, None),
    }
}
