use crate::core::error::AppResult;
use crate::core::model::Ability;
use crate::reolink::client::Client;
use crate::reolink::device;

pub fn execute(client: &Client, user_name: &str) -> AppResult<Ability> {
    device::get_ability(client, user_name)
}
