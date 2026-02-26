use crate::reolink::client::{Auth, Client};

const DEFAULT_ENDPOINT: &str = "https://camera.local";
const ENDPOINT_ENV: &str = "REOCLI_ENDPOINT";
const TOKEN_ENV: &str = "REOCLI_TOKEN";
const USER_ENV: &str = "REOCLI_USER";
const PASSWORD_ENV: &str = "REOCLI_PASSWORD";
const DEFAULT_USER: &str = "admin";

pub(crate) fn client_from_env() -> Client {
    let (primary_auth, fallback_auth) = auth_from_env();
    let client = Client::new(endpoint_from_env(), primary_auth);

    match fallback_auth {
        Some(auth) => client.with_fallback_auth(auth),
        None => client,
    }
}

pub(crate) fn ability_user_from_env() -> String {
    std::env::var(USER_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_USER.to_string())
}

fn endpoint_from_env() -> String {
    std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

fn auth_from_env() -> (Auth, Option<Auth>) {
    let user = std::env::var(USER_ENV)
        .ok()
        .map(|value| value.trim().to_string());
    let password = std::env::var(PASSWORD_ENV)
        .ok()
        .filter(|value| !value.is_empty());

    let fallback_auth = match password {
        Some(password) => {
            let user = user
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_USER.to_string());
            Some(Auth::UserPassword { user, password })
        }
        None => None,
    };

    if let Ok(token) = std::env::var(TOKEN_ENV) {
        if !token.trim().is_empty() {
            return (Auth::Token(token), fallback_auth);
        }
    }

    match fallback_auth {
        Some(user_password_auth) => (user_password_auth, None),
        None => (Auth::Anonymous, None),
    }
}
