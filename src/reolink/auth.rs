use crate::core::command::{CgiCommand, CommandParams, CommandRequest};
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::reolink::client::Client;

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

    let _ = client.execute(request)?;
    Ok(format!("token-{user_name}-{}", password.len()))
}
