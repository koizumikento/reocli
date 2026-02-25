use std::time::Duration;

use reqwest::{StatusCode, blocking::Client as HttpClient};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::core::command::{CommandParams, CommandRequest, CommandResponse};
use crate::core::error::{AppError, AppResult, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Anonymous,
    UserPassword { user: String, password: String },
    Token(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    endpoint: String,
    auth: Auth,
    allow_insecure_tls: bool,
}

impl Client {
    pub fn new(endpoint: impl Into<String>, auth: Auth) -> Self {
        Self {
            endpoint: endpoint.into(),
            auth,
            allow_insecure_tls: true,
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
            allow_insecure_tls: self.allow_insecure_tls,
        }
    }

    pub fn with_insecure_tls(self, allow_insecure_tls: bool) -> Self {
        Self {
            allow_insecure_tls,
            ..self
        }
    }

    pub fn execute(&self, request: CommandRequest) -> AppResult<CommandResponse> {
        let api_url = self.api_url()?;
        let query_params = self.query_params(request.command.as_str())?;
        let request_body = build_request_body(&request);

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

    fn query_params(&self, command: &str) -> AppResult<Vec<(&'static str, String)>> {
        let mut params = vec![("cmd", command.to_string())];

        match &self.auth {
            Auth::Anonymous => {}
            Auth::UserPassword { user, password } => {
                if user.trim().is_empty() || password.is_empty() {
                    return Err(AppError::new(
                        ErrorKind::InvalidInput,
                        "user/password auth requires non-empty credentials",
                    ));
                }
                params.push(("user", user.clone()));
                params.push(("password", password.clone()));
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
