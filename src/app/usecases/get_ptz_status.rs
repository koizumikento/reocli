use std::ops::Deref;

use crate::core::error::AppResult;
use crate::core::model::PtzStatus;
use crate::reolink::client::Client;
use crate::reolink::ptz;

#[derive(Debug, Clone, PartialEq)]
pub struct PtzStatusView {
    pub status: PtzStatus,
}

impl Deref for PtzStatusView {
    type Target = PtzStatus;

    fn deref(&self) -> &Self::Target {
        &self.status
    }
}

pub fn execute(client: &Client, channel: u8) -> AppResult<PtzStatusView> {
    let status = ptz::get_ptz_status(client, channel)?;
    Ok(PtzStatusView { status })
}
