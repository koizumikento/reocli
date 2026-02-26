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
    let normalized_iso8601 = iso8601.trim();
    if normalized_iso8601.is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "iso8601 must not be empty",
        ));
    }
    if !is_rfc3339_like_timestamp(normalized_iso8601) {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "iso8601 must be an RFC3339-like timestamp (e.g. 2026-02-25T10:00:00Z)",
        ));
    }

    let mut request = CommandRequest::new(CgiCommand::SetTime);
    request.params = CommandParams {
        user_name: None,
        channel: None,
        payload: Some(normalized_iso8601.to_string()),
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
        iso8601: normalized_iso8601.to_string(),
    })
}

fn is_rfc3339_like_timestamp(value: &str) -> bool {
    let Some((date_part, time_with_offset)) = value.split_once('T') else {
        return false;
    };
    if time_with_offset.contains('T') || !is_valid_date(date_part) {
        return false;
    }

    if let Some(time_part) = time_with_offset.strip_suffix('Z') {
        return is_valid_time(time_part);
    }

    let Some(offset_start) = find_offset_start(time_with_offset) else {
        return false;
    };

    let (time_part, offset_part) = time_with_offset.split_at(offset_start);
    is_valid_time(time_part) && is_valid_offset(offset_part)
}

fn find_offset_start(value: &str) -> Option<usize> {
    let plus = value.rfind('+');
    let minus = value.rfind('-');
    match (plus, minus) {
        (Some(plus_idx), Some(minus_idx)) => Some(plus_idx.max(minus_idx)),
        (Some(plus_idx), None) => Some(plus_idx),
        (None, Some(minus_idx)) => Some(minus_idx),
        (None, None) => None,
    }
    .filter(|index| *index > 0)
}

fn is_valid_date(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }

    let bytes = value.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    let Some(year_text) = value.get(0..4) else {
        return false;
    };
    let Some(month_text) = value.get(5..7) else {
        return false;
    };
    let Some(day_text) = value.get(8..10) else {
        return false;
    };

    let Some(year) = parse_fixed_uint(year_text, 4) else {
        return false;
    };
    let Some(month) = parse_fixed_uint(month_text, 2) else {
        return false;
    };
    let Some(day) = parse_fixed_uint(day_text, 2) else {
        return false;
    };

    if !(1..=12).contains(&month) {
        return false;
    }

    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };

    (1..=max_day).contains(&day)
}

fn is_valid_time(value: &str) -> bool {
    let (time_part, fractional_seconds) = match value.split_once('.') {
        Some((head, tail)) => (head, Some(tail)),
        None => (value, None),
    };

    if time_part.len() != 8 {
        return false;
    }

    let mut parts = time_part.split(':');
    let Some(hours) = parts
        .next()
        .and_then(|segment| parse_fixed_uint(segment, 2))
    else {
        return false;
    };
    let Some(minutes) = parts
        .next()
        .and_then(|segment| parse_fixed_uint(segment, 2))
    else {
        return false;
    };
    let Some(seconds) = parts
        .next()
        .and_then(|segment| parse_fixed_uint(segment, 2))
    else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }

    if hours > 23 || minutes > 59 || seconds > 60 {
        return false;
    }

    if let Some(fractional_seconds) = fractional_seconds
        && (fractional_seconds.is_empty()
            || !fractional_seconds
                .chars()
                .all(|character| character.is_ascii_digit()))
    {
        return false;
    }

    true
}

fn is_valid_offset(value: &str) -> bool {
    if value.len() != 6 {
        return false;
    }

    let bytes = value.as_bytes();
    if !(bytes[0] == b'+' || bytes[0] == b'-') || bytes[3] != b':' {
        return false;
    }

    let Some(hours_text) = value.get(1..3) else {
        return false;
    };
    let Some(minutes_text) = value.get(4..6) else {
        return false;
    };

    let Some(hours) = parse_fixed_uint(hours_text, 2) else {
        return false;
    };
    let Some(minutes) = parse_fixed_uint(minutes_text, 2) else {
        return false;
    };

    hours <= 23 && minutes <= 59
}

fn parse_fixed_uint(value: &str, width: usize) -> Option<u32> {
    if value.len() != width || !value.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }

    value.parse::<u32>().ok()
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
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
