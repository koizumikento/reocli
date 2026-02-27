use crate::app::usecases;
use crate::core::command::CgiCommand;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzDirection;
use crate::interfaces::runtime::{ability_user_from_env, client_from_env};
use serde_json::{Value, json};

use super::tools::supported_tools;

const DEFAULT_ABSOLUTE_RAW_TOL_COUNT: i64 = 10;
const DEFAULT_ABSOLUTE_TIMEOUT_MS: u64 = 25_000;

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
            let status_view = usecases::get_ptz_status::execute(&client, channel)?;
            let status = &status_view.status;
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
        "reolink.get_net_port" => {
            let net_port = usecases::net_port::get(&client)?;
            json_response(json!({
                "http_enable": net_port.http_enable,
                "http_port": net_port.http_port,
                "https_enable": net_port.https_enable,
                "https_port": net_port.https_port,
                "media_port": net_port.media_port,
                "onvif_enable": net_port.onvif_enable,
                "onvif_port": net_port.onvif_port,
                "rtsp_enable": net_port.rtsp_enable,
                "rtsp_port": net_port.rtsp_port,
                "rtmp_enable": net_port.rtmp_enable,
                "rtmp_port": net_port.rtmp_port
            }))
        }
        "reolink.set_onvif_enabled" => {
            let (enabled, onvif_port) = parse_set_onvif_args(&request.arguments)?;
            let net_port = usecases::net_port::set_onvif_enabled(&client, enabled, onvif_port)?;
            json_response(json!({
                "requested_enabled": enabled,
                "requested_port": onvif_port,
                "http_enable": net_port.http_enable,
                "http_port": net_port.http_port,
                "https_enable": net_port.https_enable,
                "https_port": net_port.https_port,
                "media_port": net_port.media_port,
                "onvif_enable": net_port.onvif_enable,
                "onvif_port": net_port.onvif_port,
                "rtsp_enable": net_port.rtsp_enable,
                "rtsp_port": net_port.rtsp_port,
                "rtmp_enable": net_port.rtmp_enable,
                "rtmp_port": net_port.rtmp_port
            }))
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
        "reolink.ptz_calibrate_auto" => {
            ensure_command_supported(&client, CgiCommand::PtzCtrl)?;
            let channel = parse_channel(&request.arguments)?;
            let result = usecases::ptz_calibrate_auto::execute(&client, channel)?;
            json_response(json!({
                "channel": result.channel,
                "camera_key": result.camera_key,
                "calibration_path": result.calibration_path,
                "reused_existing": result.reused_existing,
                "calibrated_state": result.calibrated_state,
                "pan_count": result.pan_count,
                "tilt_count": result.tilt_count,
                "calibration": {
                    "serial_number": result.calibration.serial_number,
                    "model": result.calibration.model,
                    "firmware": result.calibration.firmware,
                    "pan_min_count": result.calibration.pan_min_count,
                    "pan_max_count": result.calibration.pan_max_count,
                    "pan_deadband_count": result.calibration.pan_deadband_count,
                    "tilt_min_count": result.calibration.tilt_min_count,
                    "tilt_max_count": result.calibration.tilt_max_count,
                    "tilt_deadband_count": result.calibration.tilt_deadband_count,
                    "pan_model": {
                        "alpha": result.calibration.pan_model.alpha,
                        "beta": result.calibration.pan_model.beta
                    },
                    "tilt_model": {
                        "alpha": result.calibration.tilt_model.alpha,
                        "beta": result.calibration.tilt_model.beta
                    },
                    "created_at": result.calibration.created_at
                },
                "report": {
                    "samples": result.report.samples,
                    "pan_error_p95_count": result.report.pan_error_p95_count,
                    "tilt_error_p95_count": result.report.tilt_error_p95_count,
                    "notes": result.report.notes
                }
            }))
        }
        "reolink.ptz_set_absolute" => {
            ensure_command_supported(&client, CgiCommand::PtzCtrl)?;
            let (channel, pan_count, tilt_count, tol_count, timeout_ms) =
                parse_ptz_set_absolute_args(&request.arguments)?;
            let pose = usecases::ptz_set_absolute_raw::execute(
                &client, channel, pan_count, tilt_count, tol_count, timeout_ms,
            )?;
            json_response(json!({
                "channel": pose.channel,
                "pan_count": pose.pan_count,
                "tilt_count": pose.tilt_count,
                "zoom_count": pose.zoom_count,
                "focus_count": pose.focus_count,
                "tol_count": tol_count,
                "timeout_ms": timeout_ms
            }))
        }
        "reolink.ptz_get_absolute" => {
            let channel = parse_channel(&request.arguments)?;
            let pose = usecases::ptz_get_absolute_raw::execute(&client, channel)?;
            json_response(json!({
                "channel": pose.channel,
                "pan_count": pose.pan_count,
                "tilt_count": pose.tilt_count,
                "zoom_count": pose.zoom_count,
                "focus_count": pose.focus_count
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

fn parse_ptz_set_absolute_args(arguments: &[String]) -> AppResult<(u8, i64, i64, i64, u64)> {
    let (channel, consumed_channel) = parse_optional_channel_prefix(arguments)?;
    let pan_raw = arguments.get(consumed_channel).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_set_absolute requires [channel?] <pan_count> <tilt_count> [tol_count] [timeout_ms]",
        )
    })?;
    let tilt_raw = arguments.get(consumed_channel + 1).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_set_absolute requires [channel?] <pan_count> <tilt_count> [tol_count] [timeout_ms]",
        )
    })?;
    let pan_count = parse_i64(pan_raw, "pan_count")?;
    let tilt_count = parse_i64(tilt_raw, "tilt_count")?;

    let tol_index = consumed_channel + 2;
    let tol_count = match arguments.get(tol_index) {
        Some(raw) => parse_i64(raw, "tol_count")?,
        None => DEFAULT_ABSOLUTE_RAW_TOL_COUNT,
    };

    let timeout_index = tol_index + usize::from(arguments.get(tol_index).is_some());
    let timeout_ms = match arguments.get(timeout_index) {
        Some(raw) => parse_u64(raw, "timeout_ms")?,
        None => DEFAULT_ABSOLUTE_TIMEOUT_MS,
    };

    let consumed = timeout_index + usize::from(arguments.get(timeout_index).is_some());
    if consumed != arguments.len() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "reolink.ptz_set_absolute accepts [channel?] <pan_count> <tilt_count> [tol_count] [timeout_ms]",
        ));
    }

    Ok((channel, pan_count, tilt_count, tol_count, timeout_ms))
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

fn parse_i64(raw: &str, field: &str) -> AppResult<i64> {
    raw.parse::<i64>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{field} must be an integer"),
        )
    })
}

fn parse_u16(raw: &str, field: &str) -> AppResult<u16> {
    raw.parse::<u16>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{field} must be an integer between 0 and 65535"),
        )
    })
}

fn parse_bool_on_off(raw: &str, field: &str) -> AppResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "enable" | "enabled" => Ok(true),
        "0" | "false" | "off" | "disable" | "disabled" => Ok(false),
        _ => Err(AppError::new(
            ErrorKind::InvalidInput,
            format!("{field} must be one of on/off"),
        )),
    }
}

fn parse_set_onvif_args(arguments: &[String]) -> AppResult<(bool, Option<u16>)> {
    let enabled_raw = arguments.first().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "reolink.set_onvif_enabled requires <on|off> [onvif_port]",
        )
    })?;
    let enabled = parse_bool_on_off(enabled_raw, "enabled")?;
    let onvif_port = match arguments.get(1) {
        Some(raw) => Some(parse_u16(raw, "onvif_port")?),
        None => None,
    };
    if arguments.len() > 2 {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "reolink.set_onvif_enabled accepts <on|off> [onvif_port]",
        ));
    }
    Ok((enabled, onvif_port))
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
