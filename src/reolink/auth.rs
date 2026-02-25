use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::reolink::client::{Auth, Client};
use serde_json::Value;

pub fn get_user_auth(client: &Client, user_name: &str, password: &str) -> AppResult<String> {
    if user_name.trim().is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "user_name must not be empty",
        ));
    }
    if password.is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "password must not be empty",
        ));
    }

    let mut request = CommandRequest::new(CgiCommand::GetUserAuth);
    request.params = CommandParams {
        user_name: Some(user_name.to_string()),
        channel: None,
        payload: None,
    };

    let authenticated_client = client.with_auth(Auth::UserPassword {
        user: user_name.to_string(),
        password: password.to_string(),
    });
    let response = authenticated_client.execute(request)?;
    extract_token(&response.raw_json)
}

fn extract_token(raw_json: &str) -> AppResult<String> {
    let parsed: Value = serde_json::from_str(raw_json).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to parse GetUserAuth response JSON: {error}"),
        )
    })?;

    let item = response_item(&parsed)?;
    ensure_success_code(item)?;
    find_token(item).ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "GetUserAuth succeeded but token was not found in response",
        )
    })
}

fn response_item(value: &Value) -> AppResult<&Value> {
    match value {
        Value::Array(items) => items.first().ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                "GetUserAuth response array was empty",
            )
        }),
        _ => Ok(value),
    }
}

fn ensure_success_code(item: &Value) -> AppResult<()> {
    if let Some(code) = read_code(item) {
        if code != 0 {
            return Err(AppError::new(
                ErrorKind::Authentication,
                format!("GetUserAuth returned non-zero code: {code}"),
            ));
        }
    }

    Ok(())
}

fn read_code(item: &Value) -> Option<i64> {
    let value = item.get("code")?;

    if let Some(code) = value.as_i64() {
        return Some(code);
    }

    if let Some(code) = value.as_u64() {
        return i64::try_from(code).ok();
    }

    value.as_str()?.parse::<i64>().ok()
}

fn find_token(item: &Value) -> Option<String> {
    const CANDIDATE_PATHS: &[&[&str]] = &[
        &["value", "Token", "name"],
        &["value", "Token", "token"],
        &["value", "token"],
        &["value", "Token"],
        &["Token", "name"],
        &["Token", "token"],
        &["token"],
        &["Token"],
    ];

    for path in CANDIDATE_PATHS {
        if let Some(token) = value_at_path(item, path).and_then(non_empty_str) {
            return Some(token.to_string());
        }
    }

    None
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn non_empty_str(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty())
}
