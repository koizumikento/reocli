use std::path::{Path, PathBuf};

use crate::core::model::DeviceInfo;
use crate::reolink::client::{Auth, Client};

const DEFAULT_ENDPOINT: &str = "https://camera.local";
const ENDPOINT_ENV: &str = "REOCLI_ENDPOINT";
const TOKEN_ENV: &str = "REOCLI_TOKEN";
const USER_ENV: &str = "REOCLI_USER";
const PASSWORD_ENV: &str = "REOCLI_PASSWORD";
const CALIBRATION_DIR_ENV: &str = "REOCLI_CALIBRATION_DIR";
const HOME_ENV: &str = "HOME";
const DEFAULT_USER: &str = "admin";
const DEFAULT_CALIBRATION_SUBDIR: &str = ".reocli/calibration";
const UNKNOWN_KEY_COMPONENT: &str = "unknown";

pub(crate) fn client_from_env() -> Client {
    let (primary_auth, fallback_auth) = auth_from_env();
    let client = Client::new(endpoint_from_env(), primary_auth);

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

fn endpoint_from_env() -> String {
    std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

fn auth_from_env() -> (Auth, Option<Auth>) {
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

    match fallback_auth {
        Some(user_password_auth) => (user_password_auth, None),
        None => (Auth::Anonymous, None),
    }
}

fn env_var_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
