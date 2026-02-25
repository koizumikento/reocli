use crate::core::error::AppResult;
use crate::reolink::auth;
use crate::reolink::client::Client;

pub fn execute(client: &Client, user_name: &str, password: &str) -> AppResult<String> {
    auth::get_user_auth(client, user_name, password)
}
