use crate::core::error::AppResult;
use crate::core::model::SystemTime;
use crate::reolink::client::Client;
use crate::reolink::system;

pub fn execute(client: &Client, iso8601: &str) -> AppResult<SystemTime> {
    system::set_time(client, iso8601)
}
