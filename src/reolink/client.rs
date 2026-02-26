use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{StatusCode, blocking::Client as HttpClient};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::core::command::{CgiCommand, CommandParams, CommandRequest, CommandResponse};
use crate::core::error::{AppError, AppResult, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Anonymous,
    UserPassword { user: String, password: String },
    Token(String),
}

#[derive(Debug, Clone)]
pub struct Client {
    endpoint: String,
    auth: Auth,
    fallback_auth: Option<Auth>,
    allow_insecure_tls: bool,
    session_token: Arc<Mutex<Option<String>>>,
}

impl Client {
    pub fn new(endpoint: impl Into<String>, auth: Auth) -> Self {
        Self {
            endpoint: endpoint.into(),
            auth,
            fallback_auth: None,
            allow_insecure_tls: true,
            session_token: Arc::new(Mutex::new(None)),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    pub fn with_auth(&self, auth: Auth) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            auth,
            fallback_auth: self.fallback_auth.clone(),
            allow_insecure_tls: self.allow_insecure_tls,
            session_token: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_fallback_auth(&self, fallback_auth: Auth) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            auth: self.auth.clone(),
            fallback_auth: Some(fallback_auth),
            allow_insecure_tls: self.allow_insecure_tls,
            session_token: Arc::clone(&self.session_token),
        }
    }

    pub fn with_insecure_tls(self, allow_insecure_tls: bool) -> Self {
        Self {
            allow_insecure_tls,
            ..self
        }
    }

    pub fn execute(&self, request: CommandRequest) -> AppResult<CommandResponse> {
        match self.execute_with_auth(&request, &self.auth) {
            Ok(response) => Ok(response),
            Err(error) if self.should_fallback(&error) => {
                if let Some(fallback_auth) = &self.fallback_auth {
                    self.execute_with_auth(&request, fallback_auth)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn login(&self, user: &str, password: &str) -> AppResult<String> {
        if user.trim().is_empty() || password.is_empty() {
            return Err(AppError::new(
                ErrorKind::InvalidInput,
                "login requires non-empty user/password",
            ));
        }

        let payload = json!({
            "User": {
                "Version": "0",
                "userName": user,
                "password": password
            }
        });
        let mut request = CommandRequest::new(CgiCommand::Login);
        request.params = CommandParams {
            user_name: None,
            channel: None,
            payload: Some(payload.to_string()),
        };

        let response = self.execute_with_query_params(
            &request,
            vec![("cmd", CgiCommand::Login.as_str().to_string())],
        )?;
        extract_login_token(&response.raw_json)
    }

    fn should_fallback(&self, error: &AppError) -> bool {
        matches!(self.auth, Auth::Token(_))
            && self.fallback_auth.is_some()
            && matches!(error.kind, ErrorKind::Authentication)
    }

    fn execute_with_auth(
        &self,
        request: &CommandRequest,
        auth: &Auth,
    ) -> AppResult<CommandResponse> {
        match auth {
            Auth::UserPassword { user, password } => {
                if request.command == CgiCommand::Login {
                    let query_params =
                        self.query_params_for_auth(request.command.as_str(), &Auth::Anonymous)?;
                    self.execute_with_query_params(request, query_params)
                } else {
                    if let Some(cached_token) = self.cached_session_token() {
                        match self.execute_with_auth(request, &Auth::Token(cached_token)) {
                            Ok(response) => return Ok(response),
                            Err(error) if matches!(error.kind, ErrorKind::Authentication) => {
                                self.clear_session_token();
                            }
                            Err(error) => return Err(error),
                        }
                    }

                    let token = self.login(user, password)?;
                    self.set_session_token(token.clone());
                    self.execute_with_auth(request, &Auth::Token(token))
                }
            }
            _ => {
                let query_params = self.query_params_for_auth(request.command.as_str(), auth)?;
                self.execute_with_query_params(request, query_params)
            }
        }
    }

    fn execute_with_query_params(
        &self,
        request: &CommandRequest,
        query_params: Vec<(&'static str, String)>,
    ) -> AppResult<CommandResponse> {
        let api_url = self.api_url()?;
        let request_body = build_request_body(request);

        let http_client = self.http_client()?;
        let response = http_client
            .post(api_url)
            .query(&query_params)
            .json(&request_body)
            .send()
            .map_err(|error| {
                AppError::new(
                    ErrorKind::Network,
                    format!(
                        "failed to send request for {}: {error}",
                        request.command.as_str()
                    ),
                )
            })?;

        let status = response.status();
        let body_text = response.text().map_err(|error| {
            AppError::new(
                ErrorKind::Network,
                format!(
                    "failed to read response body for {}: {error}",
                    request.command.as_str()
                ),
            )
        })?;

        if status == StatusCode::UNAUTHORIZED {
            return Err(AppError::new(
                ErrorKind::Authentication,
                format!(
                    "authentication failed for {}: status={} body={}",
                    request.command.as_str(),
                    status,
                    truncate_body(&body_text)
                ),
            ));
        }

        if !status.is_success() {
            return Err(AppError::new(
                ErrorKind::Network,
                format!(
                    "request failed for {}: status={} body={}",
                    request.command.as_str(),
                    status,
                    truncate_body(&body_text)
                ),
            ));
        }

        let normalized = normalize_body(body_text);
        if looks_like_authentication_failure(&normalized) {
            return Err(AppError::new(
                ErrorKind::Authentication,
                format!(
                    "authentication failed for {}: body={}",
                    request.command.as_str(),
                    truncate_body(&normalized)
                ),
            ));
        }

        Ok(CommandResponse::new(request.command, normalized))
    }

    fn api_url(&self) -> AppResult<String> {
        let endpoint = self.endpoint.trim();
        if endpoint.is_empty() {
            return Err(AppError::new(
                ErrorKind::InvalidInput,
                "endpoint must not be empty",
            ));
        }
        Ok(format!(
            "{}/cgi-bin/api.cgi",
            endpoint.trim_end_matches('/')
        ))
    }

    fn query_params_for_auth(
        &self,
        command: &str,
        auth: &Auth,
    ) -> AppResult<Vec<(&'static str, String)>> {
        let mut params = vec![("cmd", command.to_string())];

        match auth {
            Auth::Anonymous => {}
            Auth::UserPassword { user, password } => {
                if user.trim().is_empty() || password.is_empty() {
                    return Err(AppError::new(
                        ErrorKind::InvalidInput,
                        "user/password auth requires non-empty credentials",
                    ));
                }
                return Err(AppError::new(
                    ErrorKind::InvalidInput,
                    "user/password auth must be exchanged through Login before issuing commands",
                ));
            }
            Auth::Token(token) => {
                if token.trim().is_empty() {
                    return Err(AppError::new(
                        ErrorKind::InvalidInput,
                        "token auth requires non-empty token",
                    ));
                }
                params.push(("token", token.clone()));
            }
        }

        Ok(params)
    }

    fn http_client(&self) -> AppResult<HttpClient> {
        HttpClient::builder()
            .danger_accept_invalid_certs(self.allow_insecure_tls)
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| {
                AppError::new(
                    ErrorKind::Network,
                    format!("failed to create HTTP client: {error}"),
                )
            })
    }

    fn cached_session_token(&self) -> Option<String> {
        self.session_token
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn set_session_token(&self, token: String) {
        if let Ok(mut guard) = self.session_token.lock() {
            *guard = Some(token);
        }
    }

    fn clear_session_token(&self) {
        if let Ok(mut guard) = self.session_token.lock() {
            *guard = None;
        }
    }
}

impl PartialEq for Client {
    fn eq(&self, other: &Self) -> bool {
        self.endpoint == other.endpoint
            && self.auth == other.auth
            && self.fallback_auth == other.fallback_auth
            && self.allow_insecure_tls == other.allow_insecure_tls
    }
}

impl Eq for Client {}

fn looks_like_authentication_failure(body: &str) -> bool {
    let normalized = body.to_ascii_lowercase();
    if normalized.contains("please login first") || normalized.contains("login failed") {
        return true;
    }

    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return false;
    };

    contains_rsp_code(&parsed, -6) || contains_rsp_code(&parsed, -7)
}

fn extract_login_token(raw_json: &str) -> AppResult<String> {
    let parsed: Value = serde_json::from_str(raw_json).map_err(|error| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            format!("failed to parse Login response JSON: {error}"),
        )
    })?;

    let item = response_item(&parsed)?;
    ensure_success_code(item, CgiCommand::Login)?;

    find_token(item).ok_or_else(|| {
        AppError::new(
            ErrorKind::UnexpectedResponse,
            "Login succeeded but token was not found in response",
        )
    })
}

fn response_item(value: &Value) -> AppResult<&Value> {
    match value {
        Value::Array(items) => items.first().ok_or_else(|| {
            AppError::new(ErrorKind::UnexpectedResponse, "response array was empty")
        }),
        _ => Ok(value),
    }
}

fn ensure_success_code(item: &Value, command: CgiCommand) -> AppResult<()> {
    if let Some(code) = read_code(item)
        && code != 0
    {
        return Err(AppError::new(
            ErrorKind::Authentication,
            format!("{} returned non-zero code: {code}", command.as_str()),
        ));
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

fn contains_rsp_code(value: &Value, target: i64) -> bool {
    match value {
        Value::Array(entries) => entries.iter().any(|entry| contains_rsp_code(entry, target)),
        Value::Object(map) => {
            if let Some(raw) = map.get("rspCode")
                && as_i64(raw).is_some_and(|value| value == target)
            {
                return true;
            }

            map.values().any(|nested| contains_rsp_code(nested, target))
        }
        _ => false,
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
struct CgiBody {
    cmd: String,
    action: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    param: Option<Value>,
}

fn build_request_body(request: &CommandRequest) -> Vec<CgiBody> {
    vec![CgiBody {
        cmd: request.command.as_str().to_string(),
        action: 0,
        param: build_param(&request.params),
    }]
}

fn build_param(params: &CommandParams) -> Option<Value> {
    let mut root = Map::new();

    if let Some(user_name) = &params.user_name {
        root.insert("User".to_string(), json!({ "userName": user_name }));
    }

    if let Some(channel) = params.channel {
        root.insert("channel".to_string(), json!(channel));
    }

    if let Some(payload) = &params.payload {
        match serde_json::from_str::<Value>(payload) {
            Ok(Value::Object(object)) => {
                for (key, value) in object {
                    root.insert(key, value);
                }
            }
            Ok(value) => {
                root.insert("payload".to_string(), value);
            }
            Err(_) => {
                root.insert("payload".to_string(), Value::String(payload.clone()));
            }
        }
    }

    if root.is_empty() {
        None
    } else {
        Some(Value::Object(root))
    }
}

fn normalize_body(body: String) -> String {
    match serde_json::from_str::<Value>(&body) {
        Ok(value) => serde_json::to_string(&value).unwrap_or(body),
        Err(_) => body,
    }
}

fn truncate_body(body: &str) -> String {
    const LIMIT: usize = 200;
    let mut chars = body.chars();
    let truncated = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
