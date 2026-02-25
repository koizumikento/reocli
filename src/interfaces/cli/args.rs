use crate::core::error::{AppError, AppResult, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    GetAbility { user_name: String },
    GetDevInfo,
    GetUserAuth { user_name: String, password: String },
    GetChannelStatus { channel: u8 },
    GetPtzStatus { channel: u8 },
    GetTime,
    SetTime { iso8601: String },
    Snap { channel: u8 },
    Preflight { user_name: String },
}

pub fn parse_args(args: &[String]) -> AppResult<CliCommand> {
    let Some(subcommand) = args.first() else {
        return Ok(CliCommand::Help);
    };

    match subcommand.as_str() {
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        "get-ability" => {
            let user_name = args.get(1).cloned().unwrap_or_else(|| "admin".to_string());
            Ok(CliCommand::GetAbility { user_name })
        }
        "get-dev-info" => Ok(CliCommand::GetDevInfo),
        "get-user-auth" => {
            let user_name = args.get(1).cloned().ok_or_else(|| {
                AppError::new(
                    ErrorKind::InvalidInput,
                    "get-user-auth requires <user> <password>",
                )
            })?;
            let password = args.get(2).cloned().ok_or_else(|| {
                AppError::new(
                    ErrorKind::InvalidInput,
                    "get-user-auth requires <user> <password>",
                )
            })?;
            Ok(CliCommand::GetUserAuth {
                user_name,
                password,
            })
        }
        "get-channel-status" => {
            let channel = match args.get(1) {
                Some(raw) => raw.parse::<u8>().map_err(|_| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "channel must be an integer between 0 and 255",
                    )
                })?,
                None => 0,
            };
            Ok(CliCommand::GetChannelStatus { channel })
        }
        "get-ptz-status" => {
            let channel = match args.get(1) {
                Some(raw) => raw.parse::<u8>().map_err(|_| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "channel must be an integer between 0 and 255",
                    )
                })?,
                None => 0,
            };
            Ok(CliCommand::GetPtzStatus { channel })
        }
        "get-time" => Ok(CliCommand::GetTime),
        "set-time" => {
            let iso8601 = args.get(1).cloned().ok_or_else(|| {
                AppError::new(ErrorKind::InvalidInput, "set-time requires <iso8601>")
            })?;
            Ok(CliCommand::SetTime { iso8601 })
        }
        "snap" => {
            let channel = match args.get(1) {
                Some(raw) => raw.parse::<u8>().map_err(|_| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "channel must be an integer between 0 and 255",
                    )
                })?,
                None => 0,
            };
            Ok(CliCommand::Snap { channel })
        }
        "preflight" => {
            let user_name = args.get(1).cloned().unwrap_or_else(|| "admin".to_string());
            Ok(CliCommand::Preflight { user_name })
        }
        _ => Err(AppError::new(
            ErrorKind::UnsupportedCommand,
            format!("unknown subcommand: {subcommand}"),
        )),
    }
}

pub fn help_text() -> &'static str {
    "Usage:\n  reocli help\n  reocli get-user-auth <user> <password>\n  reocli get-ability [user]\n  reocli get-dev-info\n  reocli get-channel-status [channel]\n  reocli get-ptz-status [channel]\n  reocli get-time\n  reocli set-time <iso8601>\n  reocli snap [channel]\n  reocli preflight [user]"
}
