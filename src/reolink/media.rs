use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::Snapshot;
use crate::reolink::client::Client;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub fn snap(client: &Client, channel: u8) -> AppResult<Snapshot> {
    snap_with_out_path(client, channel, None)
}

pub fn snap_with_out_path(
    client: &Client,
    channel: u8,
    out_path: Option<&str>,
) -> AppResult<Snapshot> {
    let mut request = CommandRequest::new(CgiCommand::Snap);
    request.params = CommandParams {
        user_name: None,
        channel: Some(channel),
        payload: None,
    };

    let response = client.execute(request)?;
    let image_bytes = parse_snap_response(&response.raw_json)?;
    let output_path = resolve_output_path(channel, out_path)?;
    let bytes_written = save_snapshot_file(&output_path, &image_bytes)?;
    let image_path = output_path.to_string_lossy().into_owned();

    Ok(Snapshot {
        channel,
        image_path,
        bytes_written,
    })
}

fn parse_snap_response(raw_json: &str) -> AppResult<Vec<u8>> {
    let parsed: Value = serde_json::from_str(raw_json).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("invalid Snap response JSON: {error}"),
        )
    })?;

    let entry = response_entry(&parsed);
    ensure_success_code(entry)?;
    let response_path = path_candidates(entry)
        .into_iter()
        .find_map(|candidate| candidate.and_then(non_empty_string));
    let image_bytes = extract_image_bytes(entry).ok_or_else(|| {
        let path_hint = response_path.as_deref().unwrap_or("<none>");
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!(
                "Snap response did not include image bytes in supported payload fields; parsed path={path_hint}"
            ),
        )
    })?;

    Ok(image_bytes)
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

fn resolve_output_path(channel: u8, out_path: Option<&str>) -> AppResult<PathBuf> {
    let Some(raw_path) = out_path else {
        return Ok(PathBuf::from(default_snapshot_path(channel)));
    };

    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "output path must not be empty",
        ));
    }

    Ok(PathBuf::from(trimmed))
}

fn save_snapshot_file(path: &Path, bytes: &[u8]) -> AppResult<usize> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::new(
                ErrorKind::InvalidInput,
                format!(
                    "failed to create snapshot output directory {}: {error}",
                    parent.display()
                ),
            )
        })?;
    }

    fs::write(path, bytes).map_err(|error| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("failed to write snapshot file {}: {error}", path.display()),
        )
    })?;

    Ok(bytes.len())
}

fn extract_image_bytes(entry: &Value) -> Option<Vec<u8>> {
    for candidate in byte_candidates(entry) {
        if let Some(bytes) = candidate.and_then(decode_image_bytes) {
            return Some(bytes);
        }
    }

    None
}

fn byte_candidates(entry: &Value) -> [Option<&Value>; 11] {
    [
        entry.pointer("/value/imageBase64"),
        entry.pointer("/value/base64"),
        entry.pointer("/value/b64"),
        entry.pointer("/value/data"),
        entry.pointer("/value/bytes"),
        entry.pointer("/value/imageData"),
        entry.get("imageBase64"),
        entry.get("base64"),
        entry.get("b64"),
        entry.get("data"),
        entry.get("bytes"),
    ]
}

fn decode_image_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Array(items) => decode_byte_array(items),
        Value::String(text) => decode_byte_string(text),
        Value::Object(map) => {
            const KEYS: &[&str] = &["imageBase64", "base64", "b64", "data", "bytes", "imageData"];
            for key in KEYS {
                if let Some(bytes) = map.get(*key).and_then(decode_image_bytes) {
                    return Some(bytes);
                }
            }
            None
        }
        _ => None,
    }
}

fn decode_byte_array(items: &[Value]) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(items.len());
    for item in items {
        bytes.push(item_to_byte(item)?);
    }
    Some(bytes)
}

fn item_to_byte(value: &Value) -> Option<u8> {
    if let Some(raw) = value.as_u64() {
        return u8::try_from(raw).ok();
    }

    value
        .as_str()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .and_then(|raw| u8::try_from(raw).ok())
}

fn decode_byte_string(raw: &str) -> Option<Vec<u8>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(payload) = data_url_base64_payload(trimmed)
        && let Some(decoded) = decode_base64(payload)
    {
        return Some(decoded);
    }

    if looks_like_base64(trimmed)
        && let Some(decoded) = decode_base64(trimmed)
    {
        return Some(decoded);
    }

    if !looks_like_file_path(trimmed) {
        return Some(trimmed.as_bytes().to_vec());
    }

    None
}

fn data_url_base64_payload(text: &str) -> Option<&str> {
    if !text.starts_with("data:") {
        return None;
    }

    let (meta, payload) = text.split_once(',')?;
    if meta.to_ascii_lowercase().contains(";base64") {
        Some(payload)
    } else {
        None
    }
}

fn looks_like_base64(text: &str) -> bool {
    let compact = text
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<u8>>();

    if compact.len() < 8 {
        return false;
    }

    if !compact.iter().all(|byte| is_base64_byte(*byte)) {
        return false;
    }

    if compact.contains(&b'=')
        || compact
            .iter()
            .any(|byte| matches!(byte, b'+' | b'/' | b'-' | b'_'))
    {
        return true;
    }

    compact.len() >= 16 && compact.len() % 4 == 0
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut compact = text
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<u8>>();

    if compact.is_empty() || !compact.iter().all(|byte| is_base64_byte(*byte)) {
        return None;
    }

    while compact.len() % 4 != 0 {
        compact.push(b'=');
    }

    let mut output = Vec::with_capacity((compact.len() / 4) * 3);
    for chunk in compact.chunks(4) {
        let a = base64_char_value(chunk[0])?;
        let b = base64_char_value(chunk[1])?;
        let c = base64_char_value(chunk[2]);
        let d = base64_char_value(chunk[3]);

        output.push((a << 2) | (b >> 4));

        match c {
            Some(c_value) => {
                output.push(((b & 0x0f) << 4) | (c_value >> 2));
                if let Some(d_value) = d {
                    output.push(((c_value & 0x03) << 6) | d_value);
                } else if chunk[3] != b'=' {
                    return None;
                }
            }
            None => {
                if chunk[2] != b'=' || chunk[3] != b'=' {
                    return None;
                }
            }
        }
    }

    Some(output)
}

fn is_base64_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'-' | b'_' | b'='
    )
}

fn base64_char_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        b'=' => None,
        _ => None,
    }
}

fn looks_like_file_path(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    text.contains('/')
        || text.contains('\\')
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
}
