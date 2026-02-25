use std::time::{SystemTime as StdSystemTime, UNIX_EPOCH};

use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::SystemTime;
use crate::reolink::client::Client;

pub fn get_time(client: &Client) -> AppResult<SystemTime> {
    let request = CommandRequest::new(CgiCommand::GetTime);
    let _ = client.execute(request)?;

    let unix_seconds = StdSystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::new(ErrorKind::UnexpectedResponse, "system clock error"))?
        .as_secs();

    Ok(SystemTime {
        iso8601: format!("unix:{unix_seconds}"),
    })
}

pub fn set_time(client: &Client, iso8601: &str) -> AppResult<SystemTime> {
    if iso8601.trim().is_empty() {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "iso8601 must not be empty",
        ));
    }

    let mut request = CommandRequest::new(CgiCommand::SetTime);
    request.params = CommandParams {
        user_name: None,
        channel: None,
        payload: Some(iso8601.to_string()),
    };

    let _ = client.execute(request)?;
    Ok(SystemTime {
        iso8601: iso8601.to_string(),
    })
}
