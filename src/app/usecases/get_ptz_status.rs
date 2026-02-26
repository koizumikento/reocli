use std::ops::Deref;

use crate::app::usecases::ptz_calibrate_auto;
use crate::core::error::AppResult;
use crate::core::model::PtzStatus;
use crate::reolink::client::Client;
use crate::reolink::{device, ptz};

#[derive(Debug, Clone, PartialEq)]
pub struct PtzStatusView {
    pub status: PtzStatus,
    pub pan_deg: Option<f64>,
    pub tilt_deg: Option<f64>,
    pub calibration_path: Option<String>,
}

impl Deref for PtzStatusView {
    type Target = PtzStatus;

    fn deref(&self) -> &Self::Target {
        &self.status
    }
}

pub fn execute(client: &Client, channel: u8) -> AppResult<PtzStatusView> {
    let status = ptz::get_ptz_status(client, channel)?;
    let mut pan_deg = None;
    let mut tilt_deg = None;
    let mut calibration_path = None;

    if let Ok(device_info) = device::get_dev_info(client)
        && let Ok(Some((stored, path))) =
            ptz_calibrate_auto::load_saved_params_for_device(&device_info)
        && let Ok((mapped_pan_deg, mapped_tilt_deg)) =
            ptz_calibrate_auto::map_status_to_degrees(&status, &stored.calibration)
    {
        pan_deg = Some(mapped_pan_deg);
        tilt_deg = Some(mapped_tilt_deg);
        calibration_path = Some(path.to_string_lossy().into_owned());
    }

    Ok(PtzStatusView {
        status,
        pan_deg,
        tilt_deg,
        calibration_path,
    })
}
