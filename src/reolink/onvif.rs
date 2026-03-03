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
const GET_STATUS_ACTION: &str = "http://www.onvif.org/ver20/ptz/wsdl/GetStatus";
const GET_CONFIGURATION_OPTIONS_ACTION: &str =
    "http://www.onvif.org/ver20/ptz/wsdl/GetConfigurationOptions";
const CONTINUOUS_MOVE_ACTION: &str = "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove";
const RELATIVE_MOVE_ACTION: &str = "http://www.onvif.org/ver20/ptz/wsdl/RelativeMove";
const STOP_ACTION: &str = "http://www.onvif.org/ver20/ptz/wsdl/Stop";
const GET_PROFILES_REQUEST_BODY: &str =
    "<trt:GetProfiles xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\"/>";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnvifMoveStatus {
    Idle,
    Moving,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OnvifPtzStatus {
    pub pan: Option<f64>,
    pub tilt: Option<f64>,
    pub zoom: Option<f64>,
    pub pan_tilt_move_status: Option<OnvifMoveStatus>,
    pub zoom_move_status: Option<OnvifMoveStatus>,
    pub utc_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OnvifPtzConfigurationOptions {
    pub supports_continuous_pan_tilt_velocity: bool,
    pub supports_relative_pan_tilt_translation: bool,
    pub supports_relative_pan_tilt_speed: bool,
    pub has_timeout_range: bool,
    pub timeout_min: Option<String>,
    pub timeout_max: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MediaProfile {
    token: String,
    ptz_configuration_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedServices {
    cache_key: String,
    ptz_xaddr: String,
    media_xaddr: Option<String>,
    media_profiles: Vec<MediaProfile>,
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

        if let Some(profile) = self.media_profiles.get(usize::from(channel)) {
            return Ok(profile.token.clone());
        }
        if let Some(profile) = self.media_profiles.first() {
            return Ok(profile.token.clone());
        }

        Err(AppError::new(
            ErrorKind::UnexpectedResponse,
            "ONVIF GetProfiles did not return any profile tokens".to_string(),
        ))
    }

    fn ptz_configuration_token_for(&self, profile_token: &str, channel: u8) -> Option<String> {
        self.media_profiles
            .iter()
            .find(|profile| profile.token == profile_token)
            .and_then(|profile| profile.ptz_configuration_token.clone())
            .or_else(|| {
                self.media_profiles
                    .get(usize::from(channel))
                    .and_then(|profile| profile.ptz_configuration_token.clone())
            })
            .or_else(|| {
                self.media_profiles
                    .iter()
                    .find_map(|profile| profile.ptz_configuration_token.clone())
            })
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
    let body = build_continuous_move_body(
        &profile_token,
        pan_velocity,
        tilt_velocity,
        timeout.as_deref(),
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
    let body = build_stop_body(&profile_token);
    send_soap_request(config, &resolved.ptz_xaddr, STOP_ACTION, &body)?;
    Ok(())
}

pub fn get_status(config: &OnvifConfig, channel: u8) -> AppResult<OnvifPtzStatus> {
    let resolved = resolve_services(config)?;
    let profile_token =
        resolved.profile_token_for_channel(channel, config.profile_token.as_deref())?;
    let body = build_get_status_body(&profile_token);
    let response_xml = send_soap_request(config, &resolved.ptz_xaddr, GET_STATUS_ACTION, &body)?;
    Ok(parse_get_status_response(&response_xml))
}

pub fn get_configuration_options(
    config: &OnvifConfig,
    channel: u8,
) -> AppResult<OnvifPtzConfigurationOptions> {
    let resolved = resolve_services(config)?;
    let profile_token =
        resolved.profile_token_for_channel(channel, config.profile_token.as_deref())?;
    let configuration_token =
        resolve_ptz_configuration_token(config, &resolved, channel, &profile_token)?;
    let body = build_get_configuration_options_body(&configuration_token);
    let response_xml = send_soap_request(
        config,
        &resolved.ptz_xaddr,
        GET_CONFIGURATION_OPTIONS_ACTION,
        &body,
    )?;
    Ok(parse_get_configuration_options_response(&response_xml))
}

pub fn relative_move(
    config: &OnvifConfig,
    channel: u8,
    pan_delta: f64,
    tilt_delta: f64,
    speed: u8,
) -> AppResult<()> {
    let speed = PtzSpeed::new(speed)?;
    let resolved = resolve_services(config)?;
    let profile_token =
        resolved.profile_token_for_channel(channel, config.profile_token.as_deref())?;
    let (pan_translation, tilt_translation) =
        normalize_relative_translation(pan_delta, tilt_delta, speed.value())?;
    let body = build_relative_move_body(&profile_token, pan_translation, tilt_translation);
    send_soap_request(config, &resolved.ptz_xaddr, RELATIVE_MOVE_ACTION, &body)?;
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
    let media_xaddr = extract_service_xaddr(&capabilities_xml, "media");

    let media_profiles = if config
        .profile_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty())
    {
        Vec::new()
    } else {
        let media_xaddr = media_xaddr.as_ref().ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                "ONVIF GetCapabilities response did not include Media XAddr".to_string(),
            )
        })?;
        let profiles_xml = send_soap_request(
            config,
            media_xaddr,
            GET_PROFILES_ACTION,
            GET_PROFILES_REQUEST_BODY,
        )?;
        let profiles = extract_media_profiles(&profiles_xml);
        if profiles.is_empty() {
            return Err(AppError::new(
                ErrorKind::UnexpectedResponse,
                "ONVIF GetProfiles response did not include profile tokens".to_string(),
            ));
        }
        profiles
    };

    let resolved = ResolvedServices {
        cache_key,
        ptz_xaddr,
        media_xaddr,
        media_profiles,
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

fn resolve_ptz_configuration_token(
    config: &OnvifConfig,
    resolved: &ResolvedServices,
    channel: u8,
    profile_token: &str,
) -> AppResult<String> {
    if let Some(token) = resolved.ptz_configuration_token_for(profile_token, channel) {
        return Ok(token);
    }

    let media_xaddr = resolved.media_xaddr.as_ref().ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "ONVIF GetConfigurationOptions requires Media XAddr to resolve a PTZ configuration token"
                .to_string(),
        )
    })?;
    let profiles_xml = send_soap_request(
        config,
        media_xaddr,
        GET_PROFILES_ACTION,
        GET_PROFILES_REQUEST_BODY,
    )?;
    let media_profiles = extract_media_profiles(&profiles_xml);
    media_profiles
        .iter()
        .find(|profile| profile.token == profile_token)
        .and_then(|profile| profile.ptz_configuration_token.clone())
        .or_else(|| {
            media_profiles
                .get(usize::from(channel))
                .and_then(|profile| profile.ptz_configuration_token.clone())
        })
        .or_else(|| {
            media_profiles
                .iter()
                .find_map(|profile| profile.ptz_configuration_token.clone())
        })
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "ONVIF profile '{profile_token}' did not include a PTZ configuration token"
                ),
            )
        })
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

fn normalize_relative_translation(
    pan_delta: f64,
    tilt_delta: f64,
    speed: u8,
) -> AppResult<(f64, f64)> {
    if !pan_delta.is_finite() || !tilt_delta.is_finite() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "relative move deltas must be finite numbers".to_string(),
        ));
    }
    let scale = pan_delta.abs().max(tilt_delta.abs());
    if scale == 0.0 {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "relative move requires a non-zero pan or tilt delta".to_string(),
        ));
    }

    let speed_scale = (f64::from(speed) / 64.0).clamp(1.0 / 64.0, 1.0);
    let pan = (pan_delta / scale).clamp(-1.0, 1.0) * speed_scale;
    let tilt = (tilt_delta / scale).clamp(-1.0, 1.0) * speed_scale;
    Ok((pan, tilt))
}

fn build_continuous_move_body(
    profile_token: &str,
    pan_velocity: f64,
    tilt_velocity: f64,
    timeout: Option<&str>,
) -> String {
    let timeout_xml = timeout
        .map(|value| format!("<tptz:Timeout>{value}</tptz:Timeout>"))
        .unwrap_or_default();
    format!(
        "<tptz:ContinuousMove xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ProfileToken>{}</tptz:ProfileToken>\
            <tptz:Velocity>\
              <tt:PanTilt x=\"{:.6}\" y=\"{:.6}\"/>\
            </tptz:Velocity>{timeout_xml}\
         </tptz:ContinuousMove>",
        escape_xml(profile_token),
        pan_velocity,
        tilt_velocity,
    )
}

fn build_stop_body(profile_token: &str) -> String {
    format!(
        "<tptz:Stop xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ProfileToken>{}</tptz:ProfileToken>\
            <tptz:PanTilt>true</tptz:PanTilt>\
            <tptz:Zoom>false</tptz:Zoom>\
         </tptz:Stop>",
        escape_xml(profile_token),
    )
}

fn build_get_status_body(profile_token: &str) -> String {
    format!(
        "<tptz:GetStatus xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ProfileToken>{}</tptz:ProfileToken>\
         </tptz:GetStatus>",
        escape_xml(profile_token),
    )
}

fn build_get_configuration_options_body(configuration_token: &str) -> String {
    format!(
        "<tptz:GetConfigurationOptions xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ConfigurationToken>{}</tptz:ConfigurationToken>\
         </tptz:GetConfigurationOptions>",
        escape_xml(configuration_token),
    )
}

fn build_relative_move_body(
    profile_token: &str,
    pan_translation: f64,
    tilt_translation: f64,
) -> String {
    format!(
        "<tptz:RelativeMove xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
            <tptz:ProfileToken>{}</tptz:ProfileToken>\
            <tptz:Translation>\
              <tt:PanTilt x=\"{pan_translation:.6}\" y=\"{tilt_translation:.6}\"/>\
            </tptz:Translation>\
            <tptz:Speed>\
              <tt:PanTilt x=\"{pan_translation:.6}\" y=\"{tilt_translation:.6}\"/>\
            </tptz:Speed>\
         </tptz:RelativeMove>",
        escape_xml(profile_token),
    )
}

fn extract_service_xaddr(xml: &str, service_keyword: &str) -> Option<String> {
    let keyword = service_keyword.trim().to_ascii_lowercase();
    extract_all_tag_values(xml, "tt:XAddr")
        .into_iter()
        .chain(extract_all_tag_values(xml, "XAddr"))
        .find(|value| value.to_ascii_lowercase().contains(&keyword))
}

fn extract_media_profiles(xml: &str) -> Vec<MediaProfile> {
    let mut profiles = Vec::new();
    let mut offset = 0usize;

    while let Some(tag) = find_next_open_tag(xml, &mut offset) {
        if !local_name(tag.name).eq_ignore_ascii_case("Profiles") {
            continue;
        }
        let Some(token) = extract_attribute(tag.body, "token").filter(|token| !token.is_empty())
        else {
            continue;
        };
        if profiles
            .iter()
            .any(|profile: &MediaProfile| profile.token == token)
        {
            continue;
        }

        let ptz_configuration_token = if tag.self_closing {
            None
        } else {
            let close_tag = format!("</{}>", tag.name);
            let close_start = xml[tag.content_start..]
                .find(&close_tag)
                .map(|position| tag.content_start + position);
            close_start.and_then(|close_start| {
                extract_first_tag_attribute_by_local_name(
                    &xml[tag.content_start..close_start],
                    "PTZConfiguration",
                    "token",
                )
            })
        };
        profiles.push(MediaProfile {
            token,
            ptz_configuration_token,
        });
    }

    profiles
}

fn parse_get_status_response(xml: &str) -> OnvifPtzStatus {
    let mut status = OnvifPtzStatus::default();
    if let Some(position_xml) = extract_first_tag_inner_by_local_name(xml, "Position") {
        if let Some((pan, tilt)) = extract_pan_tilt_components(&position_xml) {
            status.pan = Some(pan);
            status.tilt = Some(tilt);
        }
        if let Some(zoom) = extract_zoom_component(&position_xml) {
            status.zoom = Some(zoom);
        }
    }

    if let Some(move_status_xml) = extract_first_tag_inner_by_local_name(xml, "MoveStatus") {
        status.pan_tilt_move_status =
            extract_first_tag_text_by_local_name(&move_status_xml, "PanTilt")
                .map(|raw| parse_move_status(raw.as_str()));
        status.zoom_move_status = extract_first_tag_text_by_local_name(&move_status_xml, "Zoom")
            .map(|raw| parse_move_status(raw.as_str()));
    }

    status.utc_time =
        extract_first_tag_text_by_local_name(xml, "UtcTime").filter(|value| !value.is_empty());
    status
}

fn parse_get_configuration_options_response(xml: &str) -> OnvifPtzConfigurationOptions {
    let timeout_range_xml = extract_first_tag_inner_by_local_name(xml, "PTZTimeout");
    let timeout_min = timeout_range_xml
        .as_ref()
        .and_then(|timeout_xml| extract_first_tag_text_by_local_name(timeout_xml, "Min"))
        .or_else(|| extract_first_tag_attribute_by_local_name(xml, "PTZTimeout", "Min"));
    let timeout_max = timeout_range_xml
        .as_ref()
        .and_then(|timeout_xml| extract_first_tag_text_by_local_name(timeout_xml, "Max"))
        .or_else(|| extract_first_tag_attribute_by_local_name(xml, "PTZTimeout", "Max"));

    OnvifPtzConfigurationOptions {
        supports_continuous_pan_tilt_velocity: has_tag_with_local_name(
            xml,
            "ContinuousPanTiltVelocitySpace",
        ),
        supports_relative_pan_tilt_translation: has_tag_with_local_name(
            xml,
            "RelativePanTiltTranslationSpace",
        ),
        supports_relative_pan_tilt_speed: has_tag_with_local_name(xml, "PanTiltSpeedSpace"),
        has_timeout_range: timeout_range_xml.is_some()
            || timeout_min.is_some()
            || timeout_max.is_some(),
        timeout_min,
        timeout_max,
    }
}

fn parse_move_status(raw_status: &str) -> OnvifMoveStatus {
    match raw_status.trim().to_ascii_lowercase().as_str() {
        "idle" => OnvifMoveStatus::Idle,
        "moving" => OnvifMoveStatus::Moving,
        _ => OnvifMoveStatus::Unknown,
    }
}

fn extract_pan_tilt_components(xml: &str) -> Option<(f64, f64)> {
    let x = extract_first_tag_attribute_by_local_name(xml, "PanTilt", "x")?;
    let y = extract_first_tag_attribute_by_local_name(xml, "PanTilt", "y")?;
    let pan = x.parse::<f64>().ok()?;
    let tilt = y.parse::<f64>().ok()?;
    Some((pan, tilt))
}

fn extract_zoom_component(xml: &str) -> Option<f64> {
    extract_first_tag_attribute_by_local_name(xml, "Zoom", "x")?
        .parse::<f64>()
        .ok()
}

fn extract_first_tag_text_by_local_name(xml: &str, target_local_name: &str) -> Option<String> {
    let inner = extract_first_tag_inner_by_local_name(xml, target_local_name)?;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn extract_first_tag_inner_by_local_name(xml: &str, target_local_name: &str) -> Option<String> {
    let mut offset = 0usize;
    while let Some(tag) = find_next_open_tag(xml, &mut offset) {
        if !local_name(tag.name).eq_ignore_ascii_case(target_local_name) {
            continue;
        }
        if tag.self_closing {
            return Some(String::new());
        }

        let close_tag = format!("</{}>", tag.name);
        let close_start = xml[tag.content_start..]
            .find(&close_tag)
            .map(|position| tag.content_start + position)?;
        return Some(xml[tag.content_start..close_start].to_string());
    }
    None
}

fn extract_first_tag_attribute_by_local_name(
    xml: &str,
    target_local_name: &str,
    attribute_name: &str,
) -> Option<String> {
    let mut offset = 0usize;
    while let Some(tag) = find_next_open_tag(xml, &mut offset) {
        if local_name(tag.name).eq_ignore_ascii_case(target_local_name) {
            return extract_attribute(tag.body, attribute_name);
        }
    }
    None
}

fn has_tag_with_local_name(xml: &str, target_local_name: &str) -> bool {
    let mut offset = 0usize;
    while let Some(tag) = find_next_open_tag(xml, &mut offset) {
        if local_name(tag.name).eq_ignore_ascii_case(target_local_name) {
            return true;
        }
    }
    false
}

fn local_name(tag_name: &str) -> &str {
    tag_name.rsplit(':').next().unwrap_or(tag_name)
}

#[derive(Debug, Clone, Copy)]
struct OpenTag<'a> {
    name: &'a str,
    body: &'a str,
    content_start: usize,
    self_closing: bool,
}

fn find_next_open_tag<'a>(xml: &'a str, offset: &mut usize) -> Option<OpenTag<'a>> {
    while let Some(start_relative) = xml[*offset..].find('<') {
        let start = *offset + start_relative;
        let end = start + xml[start..].find('>')?;
        *offset = end + 1;

        let mut tag_body = xml[start + 1..end].trim();
        if tag_body.is_empty()
            || tag_body.starts_with('/')
            || tag_body.starts_with('?')
            || tag_body.starts_with('!')
        {
            continue;
        }

        let self_closing = tag_body.ends_with('/');
        if self_closing {
            tag_body = tag_body[..tag_body.len() - 1].trim_end();
        }
        let name = tag_body.split_whitespace().next()?;
        if name.is_empty() {
            continue;
        }

        return Some(OpenTag {
            name,
            body: tag_body,
            content_start: end + 1,
            self_closing,
        });
    }
    None
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
mod tests;
