use crate::app::usecases;
use crate::core::command::CgiCommand;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzDirection;
use crate::interfaces::runtime::{ability_user_from_env, client_from_env};
use serde_json::{Value, json};

use super::tools::supported_tools;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRequest {
    pub tool: String,
    pub arguments: Vec<String>,
}

pub fn handle_request(request: McpRequest) -> AppResult<String> {
    let client = client_from_env();

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
            ensure_command_supported(&client, CgiCommand::GetDevInfo)?;
            let info = usecases::get_dev_info::execute(&client)?;
            json_response(json!({
                "model": info.model,
                "firmware": info.firmware,
                "serial": info.serial_number
            }))
        }
        "reolink.get_channel_status" => {
            ensure_command_supported(&client, CgiCommand::GetChannelStatus)?;
            let channel = parse_channel(&request.arguments)?;
            let status = usecases::get_channel_status::execute(&client, channel)?;
            json_response(json!({
                "channel": status.channel,
                "online": status.online
            }))
        }
        "reolink.get_ptz_status" => {
            let channel = parse_channel(&request.arguments)?;
            let status = usecases::get_ptz_status::execute(&client, channel)?;
            json_response(json!({
                "channel": status.channel,
                "pan": status.pan_position,
                "tilt": status.tilt_position,
                "zoom": status.zoom_position,
                "focus": status.focus_position,
                "pan_range": status.pan_range.as_ref().map(|range| json!({ "min": range.min, "max": range.max })),
                "tilt_range": status.tilt_range.as_ref().map(|range| json!({ "min": range.min, "max": range.max })),
                "zoom_range": status.zoom_range.as_ref().map(|range| json!({ "min": range.min, "max": range.max })),
                "focus_range": status.focus_range.as_ref().map(|range| json!({ "min": range.min, "max": range.max })),
                "preset_range": status.preset_range.as_ref().map(|range| json!({ "min": range.min, "max": range.max })),
                "enabled_presets": status.enabled_presets,
                "calibration_state": status.calibration_state,
                "calibrated": status.calibrated()
            }))
        }
        "reolink.get_time" => {
            ensure_command_supported(&client, CgiCommand::GetTime)?;
            let time = usecases::get_time::execute(&client)?;
            json_response(json!({ "time": time.iso8601 }))
        }
        "reolink.set_time" => {
            ensure_command_supported(&client, CgiCommand::SetTime)?;
            let iso8601 = parse_iso8601(&request.arguments)?;
            let updated = usecases::set_time::execute(&client, &iso8601)?;
            json_response(json!({ "time": updated.iso8601 }))
        }
        "reolink.snap" => {
            ensure_command_supported(&client, CgiCommand::Snap)?;
            let (channel, out_path) = parse_snap_args(&request.arguments)?;
            let snapshot =
                usecases::snap::execute_with_out_path(&client, channel, out_path.as_deref())?;
            json_response(json!({
                "channel": snapshot.channel,
                "image_path": snapshot.image_path,
                "bytes_written": snapshot.bytes_written
            }))
        }
        "reolink.ptz_move" => {
            ensure_command_supported(&client, CgiCommand::PtzCtrl)?;
            let (channel, direction, speed, duration_ms) = parse_ptz_move_args(&request.arguments)?;
            usecases::ptz_move::execute(&client, channel, direction, speed, duration_ms)?;
            json_response(json!({
                "ok": true,
                "channel": channel,
                "direction": direction.as_op(),
                "speed": speed,
                "duration_ms": duration_ms
            }))
        }
        "reolink.ptz_stop" => {
            ensure_command_supported(&client, CgiCommand::PtzCtrl)?;
            let channel = parse_channel(&request.arguments)?;
            usecases::ptz_stop::execute(&client, channel)?;
            json_response(json!({ "ok": true, "channel": channel }))
        }
        "reolink.ptz_preset_list" => {
            ensure_command_supported(&client, CgiCommand::GetPtzPreset)?;
            let channel = parse_channel(&request.arguments)?;
            let presets = usecases::ptz_preset_list::execute(&client, channel)?;
            let mapped = presets
                .iter()
                .map(|preset| json!({ "id": preset.id.value(), "name": preset.name }))
                .collect::<Vec<_>>();
            json_response(json!({ "channel": channel, "presets": mapped }))
        }
        "reolink.ptz_preset_goto" => {
            ensure_command_supported(&client, CgiCommand::PtzCtrl)?;
            let (channel, preset_id) = parse_ptz_preset_goto_args(&request.arguments)?;
            usecases::ptz_preset_goto::execute(&client, channel, preset_id)?;
            json_response(json!({ "ok": true, "channel": channel, "preset_id": preset_id }))
        }
        _ => Err(AppError::new(
            ErrorKind::UnsupportedCommand,
            format!("unknown tool: {}", request.tool),
        )),
    }
}

fn parse_channel(arguments: &[String]) -> AppResult<u8> {
    match arguments.first() {
        Some(raw) => parse_u8(raw, "channel"),
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

fn parse_snap_args(arguments: &[String]) -> AppResult<(u8, Option<String>)> {
    match arguments.len() {
        0 => Ok((0, None)),
        1 => {
            if let Ok(channel) = parse_u8(&arguments[0], "channel") {
                Ok((channel, None))
            } else {
                Ok((0, Some(arguments[0].clone())))
            }
        }
        2 => Ok((
            parse_u8(&arguments[0], "channel")?,
            Some(arguments[1].clone()),
        )),
        _ => Err(AppError::new(
            ErrorKind::InvalidInput,
            "reolink.snap accepts [channel] [out_path]",
        )),
    }
}

fn parse_ptz_move_args(arguments: &[String]) -> AppResult<(u8, PtzDirection, u8, Option<u64>)> {
    let (channel, consumed_channel) = parse_optional_channel_prefix(arguments)?;
    let direction_raw = arguments.get(consumed_channel).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_move requires [channel?] <direction> [speed] [duration_ms]",
        )
    })?;
    let direction = PtzDirection::parse(direction_raw).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("unknown PTZ direction: {direction_raw}"),
        )
    })?;

    let speed_index = consumed_channel + 1;
    let speed = match arguments.get(speed_index) {
        Some(raw) => parse_u8(raw, "speed")?,
        None => 32,
    };
    let duration_index = speed_index + usize::from(arguments.get(speed_index).is_some());
    let duration_ms = match arguments.get(duration_index) {
        Some(raw) => Some(parse_u64(raw, "duration_ms")?),
        None => None,
    };
    let consumed = duration_index + usize::from(arguments.get(duration_index).is_some());
    if consumed != arguments.len() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_move accepts [channel?] <direction> [speed] [duration_ms]",
        ));
    }

    Ok((channel, direction, speed, duration_ms))
}

fn parse_ptz_preset_goto_args(arguments: &[String]) -> AppResult<(u8, u8)> {
    let (channel, consumed_channel) = parse_optional_channel_prefix(arguments)?;
    let preset_raw = arguments.get(consumed_channel).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_preset_goto requires [channel?] <preset_id>",
        )
    })?;
    if arguments.len() != consumed_channel + 1 {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_preset_goto accepts [channel?] <preset_id>",
        ));
    }

    Ok((channel, parse_u8(preset_raw, "preset_id")?))
}

fn parse_optional_channel_prefix(arguments: &[String]) -> AppResult<(u8, usize)> {
    let Some(first) = arguments.first() else {
        return Ok((0, 0));
    };
    if let Ok(channel) = parse_u8(first, "channel") {
        Ok((channel, 1))
    } else {
        Ok((0, 0))
    }
}

fn parse_u8(raw: &str, field: &str) -> AppResult<u8> {
    raw.parse::<u8>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{field} must be an integer between 0 and 255"),
        )
    })
}

fn parse_u64(raw: &str, field: &str) -> AppResult<u64> {
    raw.parse::<u64>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{field} must be a non-negative integer"),
        )
    })
}

fn ensure_command_supported(
    client: &crate::reolink::client::Client,
    command: CgiCommand,
) -> AppResult<()> {
    let user_name = ability_user_from_env();
    let ability = usecases::get_ability::execute(client, &user_name)?;
    if ability.supports(command) {
        return Ok(());
    }

    Err(AppError::new(
        ErrorKind::UnsupportedCommand,
        format!(
            "command {} is not supported by this camera (user={})",
            command.as_str(),
            ability.user_name
        ),
    ))
}

fn json_response(value: Value) -> AppResult<String> {
    serde_json::to_string(&value).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to serialize MCP response JSON: {error}"),
        )
    })
}
