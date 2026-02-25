use crate::core::error::AppResult;
use crate::core::model::DeviceInfo;
use crate::reolink::client::Client;
use crate::reolink::device;

pub fn execute(client: &Client) -> AppResult<DeviceInfo> {
    device::get_dev_info(client)
}
