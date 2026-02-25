use crate::core::command::CgiCommand;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericRange {
    pub min: i64,
    pub max: i64,
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
