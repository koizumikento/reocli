use serde::{Deserialize, Serialize};

use crate::core::command::CgiCommand;
use crate::core::error::{AppError, AppResult, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ability {
    pub user_name: String,
    pub supported_commands: Vec<CgiCommand>,
}

impl Ability {
    pub fn supports(&self, cmd: CgiCommand) -> bool {
        self.supported_commands.contains(&cmd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    pub model: String,
    pub firmware: String,
    pub serial_number: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelStatus {
    pub channel: u8,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub channel: u8,
    pub image_path: String,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemTime {
    pub iso8601: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub host: String,
    pub https_port: u16,
    pub http_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetPortSettings {
    pub http_enable: Option<bool>,
    pub http_port: Option<u16>,
    pub https_enable: Option<bool>,
    pub https_port: Option<u16>,
    pub media_port: Option<u16>,
    pub onvif_enable: Option<bool>,
    pub onvif_port: Option<u16>,
    pub rtsp_enable: Option<bool>,
    pub rtsp_port: Option<u16>,
    pub rtmp_enable: Option<bool>,
    pub rtmp_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericRange {
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PtzDirection {
    Left,
    Right,
    Up,
    Down,
    LeftUp,
    LeftDown,
    RightUp,
    RightDown,
}

impl PtzDirection {
    pub fn as_op(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::LeftUp => "LeftUp",
            Self::LeftDown => "LeftDown",
            Self::RightUp => "RightUp",
            Self::RightDown => "RightDown",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "leftup" | "left-up" => Some(Self::LeftUp),
            "leftdown" | "left-down" => Some(Self::LeftDown),
            "rightup" | "right-up" => Some(Self::RightUp),
            "rightdown" | "right-down" => Some(Self::RightDown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PtzSpeed(u8);

impl PtzSpeed {
    pub fn new(value: u8) -> AppResult<Self> {
        if (1..=64).contains(&value) {
            return Ok(Self(value));
        }

        Err(AppError::new(
            ErrorKind::InvalidInput,
            format!("speed must be in range 1..=64, got {value}"),
        ))
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresetId(u8);

impl PresetId {
    pub fn new(value: u8) -> AppResult<Self> {
        if (1..=255).contains(&value) {
            return Ok(Self(value));
        }

        Err(AppError::new(
            ErrorKind::InvalidInput,
            format!("preset_id must be in range 1..=255, got {value}"),
        ))
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtzPreset {
    pub id: PresetId,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PtzStatus {
    pub channel: u8,
    pub pan_position: Option<i64>,
    pub tilt_position: Option<i64>,
    pub zoom_position: Option<i64>,
    pub focus_position: Option<i64>,
    pub pan_range: Option<NumericRange>,
    pub tilt_range: Option<NumericRange>,
    pub zoom_range: Option<NumericRange>,
    pub focus_range: Option<NumericRange>,
    pub preset_range: Option<NumericRange>,
    pub enabled_presets: Vec<u8>,
    pub calibration_state: Option<i64>,
}

impl PtzStatus {
    pub fn calibrated(&self) -> Option<bool> {
        self.calibration_state.map(|state| state == 2)
    }

    pub fn has_data(&self) -> bool {
        self.pan_position.is_some()
            || self.tilt_position.is_some()
            || self.zoom_position.is_some()
            || self.focus_position.is_some()
            || self.pan_range.is_some()
            || self.tilt_range.is_some()
            || self.zoom_range.is_some()
            || self.focus_range.is_some()
            || self.preset_range.is_some()
            || !self.enabled_presets.is_empty()
            || self.calibration_state.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AbsolutePose {
    pub pan_deg: f64,
    pub tilt_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct AxisModelParams {
    pub alpha: f64,
    pub beta: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AxisState {
    pub position: f64,
    pub velocity: f64,
    pub bias: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AxisEstimate {
    pub state: AxisState,
    pub measured_position: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CalibrationParams {
    pub serial_number: String,
    pub model: String,
    pub firmware: String,
    pub pan_min_count: i64,
    pub pan_max_count: i64,
    pub pan_deadband_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan_deadband_increase_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan_deadband_decrease_count: Option<i64>,
    pub tilt_min_count: i64,
    pub tilt_max_count: i64,
    pub tilt_deadband_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt_deadband_increase_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt_deadband_decrease_count: Option<i64>,
    pub pan_model: AxisModelParams,
    pub tilt_model: AxisModelParams,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub samples: usize,
    pub pan_error_p95_count: i64,
    pub tilt_error_p95_count: i64,
    pub notes: String,
}
