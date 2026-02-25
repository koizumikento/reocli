use std::collections::HashSet;

use serde_json::Value;

use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{Ability, ChannelStatus, DeviceInfo};
use crate::reolink::client::Client;

pub fn get_ability(client: &Client, user_name: &str) -> AppResult<Ability> {
    let mut request = CommandRequest::new(CgiCommand::GetAbility);
    request.params = CommandParams {
        user_name: Some(user_name.to_string()),
        channel: None,
        payload: None,
    };

    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::GetAbility)?;
    let command_payload = find_command_payload(&parsed, CgiCommand::GetAbility).unwrap_or(&parsed);

    let mut detected = HashSet::new();
    collect_supported_commands(command_payload, &mut detected);

    let supported_commands = ordered_known_commands()
        .into_iter()
        .filter(|command| detected.contains(command))
        .collect::<Vec<_>>();

    if supported_commands.is_empty() {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            "GetAbility response did not include any known commands",
        ));
    }

    Ok(Ability {
        user_name: user_name.to_string(),
        supported_commands,
    })
}

pub fn get_dev_info(client: &Client) -> AppResult<DeviceInfo> {
    let request = CommandRequest::new(CgiCommand::GetDevInfo);
    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::GetDevInfo)?;
    let command_payload = find_command_payload(&parsed, CgiCommand::GetDevInfo).unwrap_or(&parsed);
    let value_payload = command_payload
        .get("value")
        .and_then(|value| value.get("DevInfo").or(Some(value)))
        .unwrap_or(command_payload);

    let model = find_string_by_keys(value_payload, &["model", "name"]);
    let firmware = find_string_by_keys(
        value_payload,
        &["firmware", "firmVer", "version", "buildVersion"],
    );
    let serial_number = find_string_by_keys(
        value_payload,
        &["serial", "serialNo", "serialNumber", "uid"],
    );

    if model.is_none() && firmware.is_none() && serial_number.is_none() {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            "GetDevInfo response did not include model/firmware/serial fields",
        ));
    }

    Ok(DeviceInfo {
        model: model.unwrap_or_default(),
        firmware: firmware.unwrap_or_default(),
        serial_number: serial_number.unwrap_or_default(),
    })
}

pub fn get_channel_status(client: &Client, channel: u8) -> AppResult<ChannelStatus> {
    let mut request = CommandRequest::new(CgiCommand::GetChannelStatus);
    request.params = CommandParams {
        user_name: None,
        channel: Some(channel),
        payload: None,
    };

    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::GetChannelStatus)?;
    let command_payload =
        find_command_payload(&parsed, CgiCommand::GetChannelStatus).unwrap_or(&parsed);
    let value_payload = command_payload.get("value").unwrap_or(command_payload);
    let online = extract_online(value_payload, channel).ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("GetChannelStatus response missing online state for channel {channel}"),
        )
    })?;

    Ok(ChannelStatus { channel, online })
}

fn ordered_known_commands() -> [CgiCommand; 8] {
    [
        CgiCommand::GetAbility,
        CgiCommand::GetDevInfo,
        CgiCommand::Snap,
        CgiCommand::GetChannelStatus,
        CgiCommand::GetTime,
        CgiCommand::SetTime,
        CgiCommand::GetNetwork,
        CgiCommand::GetUserAuth,
    ]
}

fn parse_response_json(raw_json: &str, command: CgiCommand) -> AppResult<Value> {
    serde_json::from_str(raw_json).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "failed to parse {} response JSON: {error}",
                command.as_str()
            ),
        )
    })
}

fn find_command_payload(value: &Value, command: CgiCommand) -> Option<&Value> {
    match value {
        Value::Array(entries) => entries.iter().find(|entry| {
            entry
                .get("cmd")
                .and_then(Value::as_str)
                .is_some_and(|name| name == command.as_str())
        }),
        Value::Object(_) => value
            .get("cmd")
            .and_then(Value::as_str)
            .filter(|name| *name == command.as_str())
            .map(|_| value),
        _ => None,
    }
}

fn collect_supported_commands(value: &Value, commands: &mut HashSet<CgiCommand>) {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                collect_supported_commands(entry, commands);
            }
        }
        Value::Object(map) => {
            if let Some(name) = map.get("cmd").and_then(Value::as_str)
                && let Some(command) = command_from_name(name)
                && command_enabled(value)
            {
                commands.insert(command);
            }

            for (key, nested_value) in map {
                if let Some(command) = command_from_name(key)
                    && command_enabled(nested_value)
                {
                    commands.insert(command);
                }
                collect_supported_commands(nested_value, commands);
            }
        }
        Value::String(name) => {
            if let Some(command) = command_from_name(name) {
                commands.insert(command);
            }
        }
        _ => {}
    }
}

fn command_enabled(value: &Value) -> bool {
    if let Some(enabled) = extract_permit(value) {
        return enabled;
    }

    match value {
        Value::Bool(enabled) => *enabled,
        Value::Number(number) => number.as_i64().is_none_or(|raw| raw > 0),
        Value::String(text) => parse_enabled_string(text).unwrap_or(true),
        _ => true,
    }
}

fn extract_permit(value: &Value) -> Option<bool> {
    let Value::Object(map) = value else {
        return None;
    };

    for key in ["permit", "Permit", "enabled", "enable"] {
        if let Some(raw) = map.get(key)
            && let Some(enabled) = parse_enabled_value(raw)
        {
            return Some(enabled);
        }
    }

    None
}

fn parse_enabled_value(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(enabled) => Some(*enabled),
        Value::Number(number) => number.as_i64().map(|raw| raw > 0),
        Value::String(text) => parse_enabled_string(text),
        _ => None,
    }
}

fn parse_enabled_string(text: &str) -> Option<bool> {
    let normalized = text.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "1" | "true" | "yes" | "enabled" | "on" => Some(true),
        "0" | "false" | "no" | "disabled" | "off" => Some(false),
        _ => None,
    }
}

fn command_from_name(name: &str) -> Option<CgiCommand> {
    match name {
        "GetAbility" => Some(CgiCommand::GetAbility),
        "GetDevInfo" => Some(CgiCommand::GetDevInfo),
        "Snap" => Some(CgiCommand::Snap),
        "GetChannelStatus" | "GetChannelstatus" => Some(CgiCommand::GetChannelStatus),
        "GetTime" => Some(CgiCommand::GetTime),
        "SetTime" => Some(CgiCommand::SetTime),
        "GetNetwork" => Some(CgiCommand::GetNetwork),
        "GetUserAuth" => Some(CgiCommand::GetUserAuth),
        _ => None,
    }
}

fn find_string_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (key, nested_value) in map {
                if keys
                    .iter()
                    .any(|candidate| key.eq_ignore_ascii_case(candidate))
                    && let Some(text) = value_to_text(nested_value)
                {
                    return Some(text);
                }
            }
            for nested_value in map.values() {
                if let Some(text) = find_string_by_keys(nested_value, keys) {
                    return Some(text);
                }
            }
            None
        }
        Value::Array(entries) => {
            for entry in entries {
                if let Some(text) = find_string_by_keys(entry, keys) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let normalized = text.trim();
            if normalized.is_empty() {
                None
            } else {
                Some(normalized.to_string())
            }
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn extract_online(value: &Value, channel: u8) -> Option<bool> {
    find_online_for_channel(value, channel)
}

fn find_online_for_channel(value: &Value, channel: u8) -> Option<bool> {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                if let Some(online) = find_online_for_channel(entry, channel) {
                    return Some(online);
                }
            }
            None
        }
        Value::Object(map) => {
            let channel_matches = ["channel", "channelId", "channelNo", "id"]
                .iter()
                .filter_map(|key| map.get(*key))
                .any(|raw| parse_channel(raw).is_some_and(|parsed| parsed == channel));
            if channel_matches {
                for key in ["online", "isOnline", "status", "state", "enable"] {
                    if let Some(raw) = map.get(key)
                        && let Some(online) = parse_online(raw)
                    {
                        return Some(online);
                    }
                }
            }
            for nested_value in map.values() {
                if let Some(online) = find_online_for_channel(nested_value, channel) {
                    return Some(online);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_channel(value: &Value) -> Option<u8> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|raw| u8::try_from(raw).ok()),
        Value::String(text) => text.trim().parse::<u8>().ok(),
        _ => None,
    }
}

fn parse_online(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|raw| raw != 0),
        Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "true" | "online" | "yes" | "enabled" => Some(true),
                "0" | "false" | "offline" | "no" | "disabled" => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}
