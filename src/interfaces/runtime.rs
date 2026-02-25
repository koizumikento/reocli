use crate::reolink::client::{Auth, Client};

const DEFAULT_ENDPOINT: &str = "https://camera.local";
const ENDPOINT_ENV: &str = "REOCLI_ENDPOINT";
const TOKEN_ENV: &str = "REOCLI_TOKEN";
const USER_ENV: &str = "REOCLI_USER";
const PASSWORD_ENV: &str = "REOCLI_PASSWORD";

pub(crate) fn client_from_env() -> Client {
    let (primary_auth, fallback_auth) = auth_from_env();
    let client = Client::new(endpoint_from_env(), primary_auth);

    match fallback_auth {
        Some(auth) => client.with_fallback_auth(auth),
        None => client,
    }
}

fn endpoint_from_env() -> String {
    std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

fn auth_from_env() -> (Auth, Option<Auth>) {
    let fallback_auth = match (std::env::var(USER_ENV), std::env::var(PASSWORD_ENV)) {
        (Ok(user), Ok(password)) if !user.trim().is_empty() && !password.is_empty() => {
            Some(Auth::UserPassword { user, password })
        }
        _ => None,
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
