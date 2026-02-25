use serde_json::{Value, json};

use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{NumericRange, PtzStatus};
use crate::reolink::client::Client;

pub fn get_ptz_status(client: &Client, channel: u8) -> AppResult<PtzStatus> {
    let cur_pos = execute_optional_command(client, CgiCommand::GetPtzCurPos, channel)?;
    let zoom_focus = execute_optional_command(client, CgiCommand::GetZoomFocus, channel)?;
    let presets = execute_optional_command(client, CgiCommand::GetPtzPreset, channel)?;
    let check_state = execute_optional_command(client, CgiCommand::GetPtzCheckState, channel)?;

    let mut status = PtzStatus {
        channel,
        ..PtzStatus::default()
    };

    if let Some(payload) = &cur_pos {
        apply_cur_pos_payload(payload, channel, &mut status);
    }
    if let Some(payload) = &zoom_focus {
        apply_zoom_focus_payload(payload, channel, &mut status);
    }
    if let Some(payload) = &presets {
        apply_preset_payload(payload, channel, &mut status);
    }
    if let Some(payload) = &check_state {
        apply_check_state_payload(payload, &mut status);
    }

    if !status.has_data() {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("PTZ status unavailable for channel {channel}"),
        ));
    }

    Ok(status)
}

fn execute_optional_command(
    client: &Client,
    command: CgiCommand,
    channel: u8,
) -> AppResult<Option<Value>> {
    let request = build_request(command, channel);
    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, command)?;
    let command_payload = match find_command_payload(&parsed, command) {
        Some(payload) => payload,
        None if parsed.is_object() => &parsed,
        None => return Ok(None),
    };

    if extract_code(command_payload).is_some_and(|code| code != 0) {
        return Ok(None);
    }

    Ok(Some(command_payload.clone()))
}

fn build_request(command: CgiCommand, channel: u8) -> CommandRequest {
    let mut request = CommandRequest::new(command);
    request.params = match command {
        CgiCommand::GetPtzCurPos => CommandParams {
            user_name: None,
            channel: None,
            payload: Some(json!({ "PtzCurPos": { "channel": channel } }).to_string()),
        },
        _ => CommandParams {
            user_name: None,
            channel: Some(channel),
            payload: None,
        },
    };
    request
}

fn apply_cur_pos_payload(command_payload: &Value, channel: u8, status: &mut PtzStatus) {
    let value_payload = command_payload
        .get("value")
        .and_then(|value| value.get("PtzCurPos").or(Some(value)))
        .unwrap_or(command_payload);

    status.pan_position = status
        .pan_position
        .or_else(|| find_number_by_keys(value_payload, Some(channel), &["Ppos", "pPos", "pan"]));
    status.tilt_position = status
        .tilt_position
        .or_else(|| find_number_by_keys(value_payload, Some(channel), &["Tpos", "tPos", "tilt"]));

    if let Some(range_payload) = command_payload.get("range") {
        if status.pan_range.is_none() {
            status.pan_range = find_range_by_keys(range_payload, &["Ppos", "pPos", "pan"]);
        }
        if status.tilt_range.is_none() {
            status.tilt_range = find_range_by_keys(range_payload, &["Tpos", "tPos", "tilt"]);
        }
    }
}

fn apply_zoom_focus_payload(command_payload: &Value, channel: u8, status: &mut PtzStatus) {
    let value_payload = command_payload
        .get("value")
        .and_then(|value| value.get("ZoomFocus").or(Some(value)))
        .unwrap_or(command_payload);

    status.zoom_position = status.zoom_position.or_else(|| {
        find_number_in_section(value_payload, Some(channel), &["zoom"], &["pos", "zoomPos"])
            .or_else(|| find_number_by_keys(value_payload, Some(channel), &["zoomPos", "Zpos"]))
    });
    status.focus_position = status.focus_position.or_else(|| {
        find_number_in_section(
            value_payload,
            Some(channel),
            &["focus"],
            &["pos", "focusPos"],
        )
        .or_else(|| find_number_by_keys(value_payload, Some(channel), &["focusPos"]))
    });

    if let Some(range_payload) = command_payload
        .get("range")
        .and_then(|range| range.get("ZoomFocus").or(Some(range)))
    {
        if status.zoom_range.is_none() {
            status.zoom_range = find_range_in_section(range_payload, &["zoom"])
                .or_else(|| find_range_by_keys(range_payload, &["zoomPos", "Zpos"]));
        }
        if status.focus_range.is_none() {
            status.focus_range = find_range_in_section(range_payload, &["focus"])
                .or_else(|| find_range_by_keys(range_payload, &["focusPos"]));
        }
    }
}

fn apply_preset_payload(command_payload: &Value, channel: u8, status: &mut PtzStatus) {
    let value_payload = command_payload
        .get("value")
        .and_then(|value| value.get("PtzPreset").or(Some(value)))
        .unwrap_or(command_payload);

    let mut presets = Vec::new();
    collect_enabled_preset_ids(value_payload, channel, &mut presets);
    presets.sort_unstable();
    presets.dedup();
    if !presets.is_empty() {
        status.enabled_presets = presets;
    }

    if let Some(range_payload) = command_payload.get("range") {
        if status.preset_range.is_none() {
            status.preset_range = range_payload
                .get("PtzPreset")
                .and_then(|preset| preset.get("id"))
                .and_then(parse_numeric_range)
                .or_else(|| find_range_by_keys(range_payload, &["id"]));
        }
    }
}

fn apply_check_state_payload(command_payload: &Value, status: &mut PtzStatus) {
    let value_payload = command_payload.get("value").unwrap_or(command_payload);
    status.calibration_state = status
        .calibration_state
        .or_else(|| find_number_by_keys(value_payload, None, &["PtzCheckState"]));
}

fn collect_enabled_preset_ids(value: &Value, channel: u8, presets: &mut Vec<u8>) {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                collect_enabled_preset_ids(entry, channel, presets);
            }
        }
        Value::Object(map) => {
            let channel_matches =
                !object_has_channel_identifier(map) || channel_matches_map(map, channel);
            if channel_matches
                && let (Some(id_raw), Some(enable_raw)) = (map.get("id"), map.get("enable"))
                && parse_enabled(enable_raw).is_some_and(|enabled| enabled)
                && let Some(id) = as_u8(id_raw)
            {
                presets.push(id);
            }

            for nested in map.values() {
                collect_enabled_preset_ids(nested, channel, presets);
            }
        }
        _ => {}
    }
}

fn find_number_in_section(
    value: &Value,
    channel: Option<u8>,
    section_keys: &[&str],
    value_keys: &[&str],
) -> Option<i64> {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                if let Some(found) =
                    find_number_in_section(entry, channel, section_keys, value_keys)
                {
                    return Some(found);
                }
            }
            None
        }
        Value::Object(map) => {
            for (key, nested) in map {
                if section_keys
                    .iter()
                    .any(|candidate| key.eq_ignore_ascii_case(candidate))
                    && let Some(found) = find_number_by_keys(nested, channel, value_keys)
                {
                    return Some(found);
                }
            }

            for nested in map.values() {
                if let Some(found) =
                    find_number_in_section(nested, channel, section_keys, value_keys)
                {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_number_by_keys(value: &Value, channel: Option<u8>, keys: &[&str]) -> Option<i64> {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                if let Some(found) = find_number_by_keys(entry, channel, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Object(map) => {
            let channel_matches = match channel {
                Some(ch) => !object_has_channel_identifier(map) || channel_matches_map(map, ch),
                None => true,
            };

            if channel_matches {
                for (key, nested) in map {
                    if keys
                        .iter()
                        .any(|candidate| key.eq_ignore_ascii_case(candidate))
                        && let Some(found) = as_i64(nested)
                    {
                        return Some(found);
                    }
                }
            }

            for nested in map.values() {
                if let Some(found) = find_number_by_keys(nested, channel, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_range_in_section(value: &Value, section_keys: &[&str]) -> Option<NumericRange> {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                if let Some(range) = find_range_in_section(entry, section_keys) {
                    return Some(range);
                }
            }
            None
        }
        Value::Object(map) => {
            for (key, nested) in map {
                if section_keys
                    .iter()
                    .any(|candidate| key.eq_ignore_ascii_case(candidate))
                    && let Some(range) = parse_numeric_range(nested)
                {
                    return Some(range);
                }
            }
            for nested in map.values() {
                if let Some(range) = find_range_in_section(nested, section_keys) {
                    return Some(range);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_range_by_keys(value: &Value, keys: &[&str]) -> Option<NumericRange> {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                if let Some(range) = find_range_by_keys(entry, keys) {
                    return Some(range);
                }
            }
            None
        }
        Value::Object(map) => {
            for (key, nested) in map {
                if keys
                    .iter()
                    .any(|candidate| key.eq_ignore_ascii_case(candidate))
                    && let Some(range) = parse_numeric_range(nested)
                {
                    return Some(range);
                }
            }

            for nested in map.values() {
                if let Some(range) = find_range_by_keys(nested, keys) {
                    return Some(range);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_numeric_range(value: &Value) -> Option<NumericRange> {
    match value {
        Value::Object(map) => {
            if let (Some(min), Some(max)) = (
                map.get("min").and_then(as_i64),
                map.get("max").and_then(as_i64),
            ) {
                return Some(NumericRange { min, max });
            }
            if let Some(pos) = map.get("pos").and_then(parse_numeric_range) {
                return Some(pos);
            }
            if let Some(bounds) = map.get("range").and_then(parse_numeric_range) {
                return Some(bounds);
            }
            for nested in map.values() {
                if let Some(range) = parse_numeric_range(nested) {
                    return Some(range);
                }
            }
            None
        }
        Value::Array(entries) if entries.len() >= 2 => {
            let min = as_i64(&entries[0])?;
            let max = as_i64(&entries[1])?;
            Some(NumericRange { min, max })
        }
        _ => None,
    }
}

fn object_has_channel_identifier(map: &serde_json::Map<String, Value>) -> bool {
    ["channel", "channelId", "channelNo"]
        .iter()
        .any(|key| map.contains_key(*key))
}

fn channel_matches_map(map: &serde_json::Map<String, Value>, channel: u8) -> bool {
    ["channel", "channelId", "channelNo"]
        .iter()
        .filter_map(|key| map.get(*key))
        .any(|raw| as_u8(raw).is_some_and(|parsed| parsed == channel))
}

fn parse_enabled(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|raw| raw > 0),
        Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "true" | "yes" | "enabled" | "on" => Some(true),
                "0" | "false" | "no" | "disabled" | "off" => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn as_u8(value: &Value) -> Option<u8> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|raw| u8::try_from(raw).ok()),
        Value::String(text) => text.trim().parse::<u8>().ok(),
        _ => None,
    }
}

fn extract_code(value: &Value) -> Option<i64> {
    match value {
        Value::Array(entries) => entries.iter().find_map(extract_code),
        Value::Object(map) => map.get("code").and_then(as_i64),
        _ => None,
    }
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
