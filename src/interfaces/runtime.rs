use std::path::{Path, PathBuf};

use crate::core::model::DeviceInfo;
use crate::reolink::client::{Auth, Client};

const DEFAULT_ENDPOINT: &str = "https://camera.local";
const ENDPOINT_ENV: &str = "REOCLI_ENDPOINT";
const TOKEN_ENV: &str = "REOCLI_TOKEN";
const TOKEN_CACHE_PATH_ENV: &str = "REOCLI_TOKEN_CACHE_PATH";
const USER_ENV: &str = "REOCLI_USER";
const PASSWORD_ENV: &str = "REOCLI_PASSWORD";
const CALIBRATION_DIR_ENV: &str = "REOCLI_CALIBRATION_DIR";
const PTZ_BACKEND_ENV: &str = "REOCLI_PTZ_BACKEND";
const ONVIF_DEVICE_SERVICE_URL_ENV: &str = "REOCLI_ONVIF_DEVICE_SERVICE_URL";
const ONVIF_PROFILE_TOKEN_ENV: &str = "REOCLI_ONVIF_PROFILE_TOKEN";
const ONVIF_PORT_ENV: &str = "REOCLI_ONVIF_PORT";
const PTZ_STRICT_SUCCESS_PAN_COUNT_ENV: &str = "REOCLI_PTZ_STRICT_SUCCESS_PAN_COUNT";
const PTZ_STRICT_SUCCESS_TILT_COUNT_ENV: &str = "REOCLI_PTZ_STRICT_SUCCESS_TILT_COUNT";
const HOME_ENV: &str = "HOME";
const DEFAULT_USER: &str = "admin";
const DEFAULT_TOKEN_CACHE_SUBDIR: &str = ".reocli/tokens";
const DEFAULT_CALIBRATION_SUBDIR: &str = ".reocli/calibration";
const DEFAULT_ONVIF_PORT: u16 = 8_000;
const DEFAULT_PTZ_STRICT_SUCCESS_PAN_COUNT: f64 = 50.0;
const DEFAULT_PTZ_STRICT_SUCCESS_TILT_COUNT: f64 = 24.0;
const UNKNOWN_KEY_COMPONENT: &str = "unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtzBackend {
    Cgi,
    OnvifContinuous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnvifConfig {
    pub device_service_url: String,
    pub user_name: String,
    pub password: String,
    pub profile_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PtzStrictSuccessThresholds {
    pub pan_count: f64,
    pub tilt_count: f64,
}

pub(crate) fn client_from_env() -> Client {
    let endpoint = endpoint_from_env();
    let token_cache_path = token_cache_path_from_env(&endpoint);
    let (primary_auth, fallback_auth) = auth_from_env(token_cache_path.as_deref());
    let client = Client::new(endpoint, primary_auth);
    let client = match token_cache_path {
        Some(path) => client.with_token_cache_path(path),
        None => client,
    };

    match fallback_auth {
        Some(auth) => client.with_fallback_auth(auth),
        None => client,
    }
}

pub(crate) fn ability_user_from_env() -> String {
    env_var_trimmed(USER_ENV).unwrap_or_else(|| DEFAULT_USER.to_string())
}

pub(crate) fn calibration_dir_from_env() -> PathBuf {
    if let Some(explicit_dir) = env_var_trimmed(CALIBRATION_DIR_ENV) {
        return PathBuf::from(explicit_dir);
    }

    if let Some(home) = env_var_trimmed(HOME_ENV) {
        return Path::new(&home).join(DEFAULT_CALIBRATION_SUBDIR);
    }

    PathBuf::from(DEFAULT_CALIBRATION_SUBDIR)
}

pub(crate) fn calibration_camera_key(device_info: &DeviceInfo) -> String {
    let serial = sanitize_key_component(&device_info.serial_number);
    let model = sanitize_key_component(&device_info.model);
    let firmware = sanitize_key_component(&device_info.firmware);
    format!("{serial}__{model}__{firmware}")
}

pub(crate) fn calibration_file_path_for_camera(device_info: &DeviceInfo) -> PathBuf {
    calibration_dir_from_env().join(format!("{}.json", calibration_camera_key(device_info)))
}

pub(crate) fn ptz_backend_from_env() -> PtzBackend {
    let selected = env_var_trimmed(PTZ_BACKEND_ENV)
        .unwrap_or_else(|| "cgi".to_string())
        .to_ascii_lowercase();
    match selected.as_str() {
        "onvif" | "continuous" | "onvif_continuous" | "onvif-continuous" => {
            PtzBackend::OnvifContinuous
        }
        _ => PtzBackend::Cgi,
    }
}

pub(crate) fn onvif_config_from_env() -> Option<OnvifConfig> {
    let password = env_var_trimmed(PASSWORD_ENV)?;
    let user_name = env_var_trimmed(USER_ENV).unwrap_or_else(|| DEFAULT_USER.to_string());
    let profile_token = env_var_trimmed(ONVIF_PROFILE_TOKEN_ENV);
    let endpoint = endpoint_from_env();
    let onvif_port = onvif_port_from_env();
    let device_service_url = env_var_trimmed(ONVIF_DEVICE_SERVICE_URL_ENV)
        .or_else(|| default_onvif_device_service_url(&endpoint, onvif_port))?;

    Some(OnvifConfig {
        device_service_url,
        user_name,
        password,
        profile_token,
    })
}

pub(crate) fn ptz_strict_success_thresholds_from_env() -> PtzStrictSuccessThresholds {
    PtzStrictSuccessThresholds {
        pan_count: parse_positive_f64_or_default(
            env_var_trimmed(PTZ_STRICT_SUCCESS_PAN_COUNT_ENV).as_deref(),
            DEFAULT_PTZ_STRICT_SUCCESS_PAN_COUNT,
        ),
        tilt_count: parse_positive_f64_or_default(
            env_var_trimmed(PTZ_STRICT_SUCCESS_TILT_COUNT_ENV).as_deref(),
            DEFAULT_PTZ_STRICT_SUCCESS_TILT_COUNT,
        ),
    }
}

fn endpoint_from_env() -> String {
    std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

fn onvif_port_from_env() -> u16 {
    env_var_trimmed(ONVIF_PORT_ENV)
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_ONVIF_PORT)
}

fn default_onvif_device_service_url(endpoint: &str, onvif_port: u16) -> Option<String> {
    let parsed = reqwest::Url::parse(endpoint).ok()?;
    let host = parsed.host_str()?;
    let mut url = reqwest::Url::parse("http://localhost").ok()?;
    if url.set_host(Some(host)).is_err() {
        return None;
    }
    if url.set_port(Some(onvif_port)).is_err() {
        return None;
    }
    url.set_path("/onvif/device_service");
    Some(url.to_string())
}

fn auth_from_env(token_cache_path: Option<&Path>) -> (Auth, Option<Auth>) {
    let user = env_var_trimmed(USER_ENV);
    let password = env_var_trimmed(PASSWORD_ENV);

    let fallback_auth = match password {
        Some(password) => {
            let user = user
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_USER.to_string());
            Some(Auth::UserPassword { user, password })
        }
        None => None,
    };

    if let Some(token) = env_var_trimmed(TOKEN_ENV) {
        return (Auth::Token(token), fallback_auth);
    }

    if let Some(token) = token_from_cache_file(token_cache_path) {
        return (Auth::Token(token), fallback_auth);
    }

    match fallback_auth {
        Some(user_password_auth) => (user_password_auth, None),
        None => (Auth::Anonymous, None),
    }
}

fn token_cache_path_from_env(endpoint: &str) -> Option<PathBuf> {
    if let Some(explicit_path) = env_var_trimmed(TOKEN_CACHE_PATH_ENV) {
        return Some(PathBuf::from(explicit_path));
    }

    let home = env_var_trimmed(HOME_ENV)?;
    let endpoint_key = sanitize_key_component(endpoint);
    Some(
        Path::new(&home)
            .join(DEFAULT_TOKEN_CACHE_SUBDIR)
            .join(format!("{endpoint_key}.token")),
    )
}

fn token_from_cache_file(token_cache_path: Option<&Path>) -> Option<String> {
    let path = token_cache_path?;
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_var_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_positive_f64_or_default(raw: Option<&str>, default_value: f64) -> f64 {
    raw.and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(default_value)
}

fn sanitize_key_component(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut previous_was_separator = false;

    for character in raw.trim().chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            normalized.push('_');
            previous_was_separator = true;
        }
    }

    let trimmed = normalized.trim_matches('_');
    if trimmed.is_empty() {
        UNKNOWN_KEY_COMPONENT.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_positive_f64_or_default;

    #[test]
    fn parse_positive_f64_or_default_accepts_positive_finite_values() {
        assert_eq!(parse_positive_f64_or_default(Some("50"), 24.0), 50.0);
        assert_eq!(parse_positive_f64_or_default(Some("24.5"), 10.0), 24.5);
    }

    #[test]
    fn parse_positive_f64_or_default_falls_back_on_invalid_values() {
        assert_eq!(parse_positive_f64_or_default(None, 24.0), 24.0);
        assert_eq!(parse_positive_f64_or_default(Some(""), 24.0), 24.0);
        assert_eq!(parse_positive_f64_or_default(Some("abc"), 24.0), 24.0);
        assert_eq!(parse_positive_f64_or_default(Some("0"), 24.0), 24.0);
        assert_eq!(parse_positive_f64_or_default(Some("-1"), 24.0), 24.0);
        assert_eq!(parse_positive_f64_or_default(Some("NaN"), 24.0), 24.0);
        assert_eq!(parse_positive_f64_or_default(Some("inf"), 24.0), 24.0);
    }
}
