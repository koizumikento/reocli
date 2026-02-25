use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::Snapshot;
use crate::reolink::client::Client;
use serde_json::Value;

pub fn snap(client: &Client, channel: u8) -> AppResult<Snapshot> {
    let mut request = CommandRequest::new(CgiCommand::Snap);
    request.params = CommandParams {
        user_name: None,
        channel: Some(channel),
        payload: None,
    };

    let response = client.execute(request)?;
    let image_path = extract_image_path_from_response(&response.raw_json)?
        .unwrap_or_else(|| default_snapshot_path(channel));

    Ok(Snapshot {
        channel,
        image_path,
    })
}

fn extract_image_path_from_response(raw_json: &str) -> AppResult<Option<String>> {
    let parsed: Value = serde_json::from_str(raw_json).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("invalid Snap response JSON: {error}"),
        )
    })?;

    let entry = response_entry(&parsed);
    ensure_success_code(entry)?;
    Ok(path_candidates(entry)
        .into_iter()
        .find_map(|candidate| candidate.and_then(non_empty_string)))
}

fn response_entry(parsed: &Value) -> &Value {
    parsed
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(parsed)
}

fn ensure_success_code(entry: &Value) -> AppResult<()> {
    let Some(code) = entry.get("code").and_then(to_i64) else {
        return Ok(());
    };

    if code == 0 {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("Snap command failed with code={code}"),
        ))
    }
}

fn to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<i64>().ok()))
}

fn path_candidates(entry: &Value) -> [Option<&Value>; 5] {
    [
        entry.pointer("/value/path"),
        entry.pointer("/value/file"),
        entry.get("file"),
        entry.get("path"),
        entry.get("image"),
    ]
}

fn non_empty_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

fn default_snapshot_path(channel: u8) -> String {
    format!("snapshots/channel-{channel}.jpg")
}
