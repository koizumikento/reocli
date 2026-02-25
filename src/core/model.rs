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
