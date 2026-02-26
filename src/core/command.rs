#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CgiCommand {
    Login,
    GetAbility,
    GetDevInfo,
    Snap,
    GetChannelStatus,
    GetPtzCurPos,
    GetPtzPreset,
    GetPtzCheckState,
    PtzCtrl,
    GetZoomFocus,
    GetTime,
    SetTime,
    GetNetwork,
    GetUserAuth,
}

impl CgiCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "Login",
            Self::GetAbility => "GetAbility",
            Self::GetDevInfo => "GetDevInfo",
            Self::Snap => "Snap",
            Self::GetChannelStatus => "GetChannelStatus",
            Self::GetPtzCurPos => "GetPtzCurPos",
            Self::GetPtzPreset => "GetPtzPreset",
            Self::GetPtzCheckState => "GetPtzCheckState",
            Self::PtzCtrl => "PtzCtrl",
            Self::GetZoomFocus => "GetZoomFocus",
            Self::GetTime => "GetTime",
            Self::SetTime => "SetTime",
            Self::GetNetwork => "GetNetwork",
            Self::GetUserAuth => "GetUserAuth",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandParams {
    pub user_name: Option<String>,
    pub channel: Option<u8>,
    pub payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub command: CgiCommand,
    pub params: CommandParams,
}

impl CommandRequest {
    pub fn new(command: CgiCommand) -> Self {
        Self {
            command,
            params: CommandParams::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResponse {
    pub command: CgiCommand,
    pub raw_json: String,
}

impl CommandResponse {
    pub fn new(command: CgiCommand, raw_json: impl Into<String>) -> Self {
        Self {
            command,
            raw_json: raw_json.into(),
        }
    }
}
