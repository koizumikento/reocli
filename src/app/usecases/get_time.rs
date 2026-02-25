use crate::core::error::AppResult;
use crate::core::model::SystemTime;
use crate::reolink::client::Client;
use crate::reolink::system;

pub fn execute(client: &Client) -> AppResult<SystemTime> {
    system::get_time(client)
}
