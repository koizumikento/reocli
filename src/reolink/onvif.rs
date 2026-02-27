use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use reqwest::blocking::Client as HttpClient;
use sha1::{Digest, Sha1};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::{PtzDirection, PtzSpeed};

const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 5_000;
const GET_CAPABILITIES_ACTION: &str = "http://www.onvif.org/ver10/device/wsdl/GetCapabilities";
const GET_PROFILES_ACTION: &str = "http://www.onvif.org/ver10/media/wsdl/GetProfiles";
const CONTINUOUS_MOVE_ACTION: &str = "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove";
const STOP_ACTION: &str = "http://www.onvif.org/ver20/ptz/wsdl/Stop";
const MAX_ERROR_BODY_CHARS: usize = 360;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnvifConfig {
    pub device_service_url: String,
    pub user_name: String,
    pub password: String,
    pub profile_token: Option<String>,
    pub allow_insecure_tls: bool,
    pub request_timeout_ms: u64,
}

impl OnvifConfig {
    pub fn with_defaults(
        device_service_url: impl Into<String>,
        user_name: impl Into<String>,
        password: impl Into<String>,
        profile_token: Option<String>,
    ) -> Self {
        Self {
            device_service_url: device_service_url.into(),
            user_name: user_name.into(),
            password: password.into(),
            profile_token,
            allow_insecure_tls: true,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedServices {
    cache_key: String,
    ptz_xaddr: String,
    profile_tokens: Vec<String>,
}

impl ResolvedServices {
    fn profile_token_for_channel(
        &self,
        channel: u8,
        explicit_token: Option<&str>,
    ) -> AppResult<String> {
        if let Some(token) = explicit_token
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            return Ok(token.to_string());
        }

        if let Some(token) = self.profile_tokens.get(usize::from(channel)) {
            return Ok(token.clone());
        }
        if let Some(token) = self.profile_tokens.first() {
            return Ok(token.clone());
        }

        Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            "ONVIF GetProfiles did not return any profile tokens".to_string(),
        ))
    }
}

pub fn continuous_move(
    config: &OnvifConfig,
    channel: u8,
    direction: PtzDirection,
    speed: u8,
    duration_ms: Option<u64>,
) -> AppResult<()> {
    let speed = PtzSpeed::new(speed)?;
    let (pan_velocity, tilt_velocity) = velocity_from_direction(direction, speed.value())?;
    let resolved = resolve_services(config)?;
    let profile_token =
        resolved.profile_token_for_channel(channel, config.profile_token.as_deref())?;
    let timeout = duration_ms.map(format_duration);

    let body = format!(
        "<tptz:ContinuousMove xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ProfileToken>{}</tptz:ProfileToken>\
            <tptz:Velocity>\
              <tt:PanTilt x=\"{:.6}\" y=\"{:.6}\"/>\
            </tptz:Velocity>{}\
         </tptz:ContinuousMove>",
        escape_xml(&profile_token),
        pan_velocity,
        tilt_velocity,
        timeout
            .as_deref()
            .map(|value| format!("<tptz:Timeout>{value}</tptz:Timeout>"))
            .unwrap_or_default(),
    );
    send_soap_request(config, &resolved.ptz_xaddr, CONTINUOUS_MOVE_ACTION, &body)?;

    if let Some(duration_ms) = duration_ms {
        if duration_ms > 0 {
            thread::sleep(Duration::from_millis(duration_ms));
        }
        stop(config, channel)?;
    }

    Ok(())
}

pub fn stop(config: &OnvifConfig, channel: u8) -> AppResult<()> {
    let resolved = resolve_services(config)?;
    let profile_token =
        resolved.profile_token_for_channel(channel, config.profile_token.as_deref())?;

    let body = format!(
        "<tptz:Stop xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ProfileToken>{}</tptz:ProfileToken>\
            <tptz:PanTilt>true</tptz:PanTilt>\
            <tptz:Zoom>false</tptz:Zoom>\
         </tptz:Stop>",
        escape_xml(&profile_token),
    );
    send_soap_request(config, &resolved.ptz_xaddr, STOP_ACTION, &body)?;
    Ok(())
}

fn resolve_services(config: &OnvifConfig) -> AppResult<ResolvedServices> {
    validate_config(config)?;
    let cache_key = build_cache_key(config);

    {
        let guard = resolved_services_cache().lock().map_err(|_| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                "ONVIF service cache lock is poisoned".to_string(),
            )
        })?;
        if let Some(cached) = guard.as_ref()
            && cached.cache_key == cache_key
        {
            return Ok(cached.clone());
        }
    }

    let capabilities_body = "<tds:GetCapabilities xmlns:tds=\"http://www.onvif.org/ver10/device/wsdl\">\
         <tds:Category>PTZ</tds:Category>\
         <tds:Category>Media</tds:Category>\
       </tds:GetCapabilities>";
    let capabilities_xml = send_soap_request(
        config,
        &config.device_service_url,
        GET_CAPABILITIES_ACTION,
        capabilities_body,
    )?;
    let ptz_xaddr = extract_service_xaddr(&capabilities_xml, "ptz").ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "ONVIF GetCapabilities response did not include PTZ XAddr".to_string(),
        )
    })?;

    let profile_tokens = if config
        .profile_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty())
    {
        Vec::new()
    } else {
        let media_xaddr = extract_service_xaddr(&capabilities_xml, "media").ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                "ONVIF GetCapabilities response did not include Media XAddr".to_string(),
            )
        })?;
        let profiles_xml = send_soap_request(
            config,
            &media_xaddr,
            GET_PROFILES_ACTION,
            "<trt:GetProfiles xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\"/>",
        )?;
        let tokens = extract_profile_tokens(&profiles_xml);
        if tokens.is_empty() {
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                "ONVIF GetProfiles response did not include profile tokens".to_string(),
            ));
        }
        tokens
    };

    let resolved = ResolvedServices {
        cache_key,
        ptz_xaddr,
        profile_tokens,
    };
    let mut guard = resolved_services_cache().lock().map_err(|_| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "ONVIF service cache lock is poisoned".to_string(),
        )
    })?;
    *guard = Some(resolved.clone());

    Ok(resolved)
}

fn validate_config(config: &OnvifConfig) -> AppResult<()> {
    if config.device_service_url.trim().is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ONVIF device service URL must not be empty".to_string(),
        ));
    }
    if config.user_name.trim().is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ONVIF user_name must not be empty".to_string(),
        ));
    }
    if config.password.is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ONVIF password must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn build_cache_key(config: &OnvifConfig) -> String {
    format!(
        "{}|{}",
        config.device_service_url,
        config
            .profile_token
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
    )
}

fn resolved_services_cache() -> &'static Mutex<Option<ResolvedServices>> {
    static CACHE: OnceLock<Mutex<Option<ResolvedServices>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn send_soap_request(
    config: &OnvifConfig,
    url: &str,
    action: &str,
    operation_body: &str,
) -> AppResult<String> {
    let security_header = build_security_header(&config.user_name, &config.password)?;
    let envelope = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
                     xmlns:wsse=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd\" \
                     xmlns:wsu=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd\" \
                     xmlns:tt=\"http://www.onvif.org/ver10/schema\">\
           <s:Header>{security_header}</s:Header>\
           <s:Body>{operation_body}</s:Body>\
         </s:Envelope>"
    );

    let client = HttpClient::builder()
        .timeout(Duration::from_millis(config.request_timeout_ms.max(1)))
        .danger_accept_invalid_certs(config.allow_insecure_tls)
        .build()
        .map_err(|error| {
            AppError::new(
                ErrorKind::Network,
                format!("failed to build ONVIF HTTP client: {error}"),
            )
        })?;
    let content_type = format!("application/soap+xml; charset=utf-8; action=\"{action}\"");
    let response = client
        .post(url)
        .header("Content-Type", content_type)
        .body(envelope)
        .send()
        .map_err(|error| {
            AppError::new(
                ErrorKind::Network,
                format!("failed ONVIF SOAP request {action}: {error}"),
            )
        })?;
    let status = response.status();
    let body = response.text().map_err(|error| {
        AppError::new(
            ErrorKind::Network,
            format!("failed to read ONVIF SOAP response body for {action}: {error}"),
        )
    })?;

    if !status.is_success() {
        return Err(AppError::new(
            ErrorKind::Network,
            format!(
                "ONVIF SOAP request {action} failed: status={} body={}",
                status,
                truncate_body(&body)
            ),
        ));
    }
    if let Some(fault) = extract_fault_message(&body) {
        return Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("ONVIF SOAP fault for {action}: {fault}"),
        ));
    }

    Ok(body)
}

fn build_security_header(user_name: &str, password: &str) -> AppResult<String> {
    let created = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!("failed to format ONVIF created timestamp: {error}"),
            )
        })?;
    let nonce = generate_nonce();
    let digest = password_digest(&nonce, &created, password);
    let nonce_text = BASE64_STANDARD.encode(nonce);

    Ok(format!(
        "<wsse:Security s:mustUnderstand=\"1\">\
           <wsse:UsernameToken>\
             <wsse:Username>{}</wsse:Username>\
             <wsse:Password Type=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest\">{digest}</wsse:Password>\
             <wsse:Nonce EncodingType=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary\">{nonce_text}</wsse:Nonce>\
             <wsu:Created>{created}</wsu:Created>\
           </wsse:UsernameToken>\
         </wsse:Security>",
        escape_xml(user_name)
    ))
}

fn generate_nonce() -> [u8; 20] {
    let mut nonce = [0u8; 20];
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nonce[..16].copy_from_slice(&epoch_nanos.to_be_bytes());
    nonce[16..].copy_from_slice(&std::process::id().to_be_bytes());
    nonce
}

fn password_digest(nonce: &[u8], created: &str, password: &str) -> String {
    let mut digest = Sha1::new();
    digest.update(nonce);
    digest.update(created.as_bytes());
    digest.update(password.as_bytes());
    BASE64_STANDARD.encode(digest.finalize())
}

fn velocity_from_direction(direction: PtzDirection, speed: u8) -> AppResult<(f64, f64)> {
    let velocity = (f64::from(speed) / 64.0).clamp(1.0 / 64.0, 1.0);
    match direction {
        PtzDirection::Left => Ok((-velocity, 0.0)),
        PtzDirection::Right => Ok((velocity, 0.0)),
        PtzDirection::Up => Ok((0.0, velocity)),
        PtzDirection::Down => Ok((0.0, -velocity)),
        PtzDirection::LeftUp
        | PtzDirection::LeftDown
        | PtzDirection::RightUp
        | PtzDirection::RightDown => Err(AppError::new(
            ErrorKind::InvalidInput,
            "ONVIF ContinuousMove backend does not support diagonal PTZ directions".to_string(),
        )),
    }
}

fn extract_service_xaddr(xml: &str, service_keyword: &str) -> Option<String> {
    let keyword = service_keyword.trim().to_ascii_lowercase();
    extract_all_tag_values(xml, "tt:XAddr")
        .into_iter()
        .chain(extract_all_tag_values(xml, "XAddr"))
        .find(|value| value.to_ascii_lowercase().contains(&keyword))
}

fn extract_profile_tokens(xml: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut offset = 0usize;

    while let Some(tag_start_rel) = xml[offset..].find('<') {
        let tag_start = offset + tag_start_rel;
        let Some(tag_end_rel) = xml[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + tag_end_rel;
        let tag_body = xml[tag_start + 1..tag_end].trim();
        if !tag_body.starts_with('/')
            && tag_body.contains("Profiles")
            && let Some(token) = extract_attribute(tag_body, "token")
            && !token.is_empty()
            && !tokens.contains(&token)
        {
            tokens.push(token);
        }
        offset = tag_end + 1;
    }

    tokens
}

fn extract_attribute(tag_body: &str, attribute_name: &str) -> Option<String> {
    let needle = format!("{attribute_name}=");
    let value_start = tag_body.find(&needle)? + needle.len();
    let remainder = &tag_body[value_start..];
    let quote = remainder.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let remainder = &remainder[quote.len_utf8()..];
    let value_end = remainder.find(quote)?;
    Some(remainder[..value_end].to_string())
}

fn extract_fault_message(xml: &str) -> Option<String> {
    if !xml.contains(":Fault") && !xml.contains("<Fault>") {
        return None;
    }

    for tag in ["s:Text", "soap:Text", "faultstring", "s:Reason", "Reason"] {
        if let Some(text) = extract_all_tag_values(xml, tag)
            .into_iter()
            .find(|value| !value.trim().is_empty())
        {
            return Some(text.trim().to_string());
        }
    }

    Some("unknown SOAP fault".to_string())
}

fn extract_all_tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open_tag = format!("<{tag}>");
    let close_tag = format!("</{tag}>");
    let mut values = Vec::new();
    let mut offset = 0usize;

    while let Some(open_rel) = xml[offset..].find(&open_tag) {
        let start = offset + open_rel + open_tag.len();
        let Some(close_rel) = xml[start..].find(&close_tag) else {
            break;
        };
        let end = start + close_rel;
        values.push(xml[start..end].trim().to_string());
        offset = end + close_tag.len();
    }

    values
}

fn truncate_body(body: &str) -> String {
    if body.chars().count() <= MAX_ERROR_BODY_CHARS {
        return body.to_string();
    }
    body.chars().take(MAX_ERROR_BODY_CHARS).collect::<String>() + "..."
}

fn format_duration(duration_ms: u64) -> String {
    let seconds = duration_ms / 1_000;
    let milliseconds = duration_ms % 1_000;
    if milliseconds == 0 {
        format!("PT{seconds}S")
    } else {
        format!("PT{seconds}.{milliseconds:03}S")
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_profile_tokens_parses_multiple_entries() {
        let xml = r#"
        <trt:GetProfilesResponse xmlns:trt="http://www.onvif.org/ver10/media/wsdl">
          <trt:Profiles token="Profile000"></trt:Profiles>
          <trt:Profiles token='Profile001'></trt:Profiles>
          <trt:Profiles></trt:Profiles>
        </trt:GetProfilesResponse>
        "#;
        let tokens = extract_profile_tokens(xml);
        assert_eq!(
            tokens,
            vec!["Profile000".to_string(), "Profile001".to_string()]
        );
    }

    #[test]
    fn velocity_from_direction_rejects_diagonal() {
        let error = velocity_from_direction(PtzDirection::LeftUp, 8).expect_err("must fail");
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert!(error.message.contains("diagonal"));
    }
}
