use serde_json::Value;

use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::SystemTime;
use crate::reolink::client::Client;

pub fn get_time(client: &Client) -> AppResult<SystemTime> {
    let request = CommandRequest::new(CgiCommand::GetTime);
    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::GetTime)?;
    let command_payload = find_command_payload(&parsed, CgiCommand::GetTime).unwrap_or(&parsed);
    let value_payload = command_payload.get("value").unwrap_or(command_payload);
    let iso8601 = find_time_text(value_payload).ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "GetTime response did not include a time string",
        )
    })?;

    Ok(SystemTime { iso8601 })
}

pub fn set_time(client: &Client, iso8601: &str) -> AppResult<SystemTime> {
    if iso8601.trim().is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "iso8601 must not be empty",
        ));
    }

    let mut request = CommandRequest::new(CgiCommand::SetTime);
    request.params = CommandParams {
        user_name: None,
        channel: None,
        payload: Some(iso8601.to_string()),
    };

    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::SetTime)?;
    let command_payload = find_command_payload(&parsed, CgiCommand::SetTime).unwrap_or(&parsed);
    let code = extract_code(command_payload).ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "SetTime response did not include result code",
        )
    })?;
    if code != 0 {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("SetTime failed with code={code}"),
        ));
    }

    Ok(SystemTime {
        iso8601: iso8601.to_string(),
    })
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

fn find_time_text(value: &Value) -> Option<String> {
    find_string_by_keys(
        value,
        &[
            "time",
            "localTime",
            "iso8601",
            "dateTime",
            "timeStr",
            "utcTime",
        ],
    )
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

fn extract_code(value: &Value) -> Option<i64> {
    value.get("code").and_then(|code| match code {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    })
}
