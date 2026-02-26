use crate::app::usecases::ptz_calibrate_auto;
use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::reolink::client::Client;
use crate::reolink::{device, ptz};

#[derive(Debug, Clone, PartialEq)]
pub struct PtzAbsolutePosition {
    pub channel: u8,
    pub pan_deg: f64,
    pub tilt_deg: f64,
    pub calibration_path: String,
}

pub fn execute(client: &Client, channel: u8) -> AppResult<PtzAbsolutePosition> {
    let device_info = device::get_dev_info(client)?;
    let (stored_params, calibration_path) =
        ptz_calibrate_auto::load_saved_params_for_device(&device_info)?.ok_or_else(|| {
            AppError::new(
                ErrorKind::UnexpectedResponse,
                format!(
                    "missing PTZ calibration for camera key {}; run calibrate_auto first",
                    crate::interfaces::runtime::calibration_camera_key(&device_info)
                ),
            )
        })?;

    let status = ptz::get_ptz_cur_pos(client, channel)?;
    let (pan_deg, tilt_deg) =
        ptz_calibrate_auto::map_status_to_degrees(&status, &stored_params.calibration)?;

    Ok(PtzAbsolutePosition {
        channel,
        pan_deg,
        tilt_deg,
        calibration_path: calibration_path.to_string_lossy().into_owned(),
    })
}
