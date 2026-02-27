use serde_json::{Value, json};

use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::NetPortSettings;
use crate::reolink::client::Client;

pub fn get_net_port(client: &Client) -> AppResult<NetPortSettings> {
    let mut request = CommandRequest::new(CgiCommand::GetNetPort);
    request.params = CommandParams {
        user_name: None,
        channel: None,
        payload: Some(json!({ "NetPort": {} }).to_string()),
    };

    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::GetNetPort)?;
    let payload = find_command_payload(&parsed, CgiCommand::GetNetPort).unwrap_or(&parsed);
    ensure_code_ok(payload, CgiCommand::GetNetPort)?;

    let value_payload = payload.get("value").unwrap_or(payload);
    let net_port = value_payload
        .get("NetPort")
        .or_else(|| value_payload.get("netPort"))
        .unwrap_or(value_payload);

    Ok(NetPortSettings {
        http_enable: find_bool_by_keys(net_port, &["httpEnable"]),
        http_port: find_u16_by_keys(net_port, &["httpPort"]),
        https_enable: find_bool_by_keys(net_port, &["httpsEnable"]),
        https_port: find_u16_by_keys(net_port, &["httpsPort"]),
        media_port: find_u16_by_keys(net_port, &["mediaPort"]),
        onvif_enable: find_bool_by_keys(net_port, &["onvifEnable"]),
        onvif_port: find_u16_by_keys(net_port, &["onvifPort"]),
        rtsp_enable: find_bool_by_keys(net_port, &["rtspEnable"]),
        rtsp_port: find_u16_by_keys(net_port, &["rtspPort"]),
        rtmp_enable: find_bool_by_keys(net_port, &["rtmpEnable"]),
        rtmp_port: find_u16_by_keys(net_port, &["rtmpPort"]),
    })
}

pub fn set_onvif_enabled(
    client: &Client,
    enabled: bool,
    onvif_port: Option<u16>,
) -> AppResult<NetPortSettings> {
    let current = get_net_port(client)?;
    let target_port = onvif_port.or(current.onvif_port).unwrap_or(8000);

    let mut request = CommandRequest::new(CgiCommand::SetNetPort);
    request.action = 1;
    request.params = CommandParams {
        user_name: None,
        channel: None,
        payload: Some(
            json!({
                "NetPort": {
                    "onvifEnable": if enabled { 1 } else { 0 },
                    "onvifPort": target_port
                }
            })
            .to_string(),
        ),
    };

    let response = client.execute(request)?;
    let parsed = parse_response_json(&response.raw_json, CgiCommand::SetNetPort)?;
    let payload = find_command_payload(&parsed, CgiCommand::SetNetPort).unwrap_or(&parsed);
    ensure_code_ok(payload, CgiCommand::SetNetPort)?;

    let rsp_code = payload
        .pointer("/value/rspCode")
        .and_then(as_i64)
        .unwrap_or(200);
    if rsp_code != 200 {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("SetNetPort returned rspCode={rsp_code}"),
        ));
    }

    get_net_port(client)
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

fn ensure_code_ok(payload: &Value, command: CgiCommand) -> AppResult<()> {
    let code = payload.get("code").and_then(as_i64).ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("{} response did not include code", command.as_str()),
        )
    })?;
    if code == 0 {
        return Ok(());
    }

    Err(AppError::new(
        ErrorKind::UnexpectedResponse,
        format!("{} failed with code={code}", command.as_str()),
    ))
}

fn find_bool_by_keys(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(parse_bool)
}

fn find_u16_by_keys(value: &Value, keys: &[&str]) -> Option<u16> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(as_u16)
}

fn parse_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|raw| raw != 0),
        Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "true" | "yes" | "on" | "enabled" => Some(true),
                "0" | "false" | "no" | "off" | "disabled" => Some(false),
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

fn as_u16(value: &Value) -> Option<u16> {
    as_i64(value).and_then(|raw| u16::try_from(raw).ok())
}
