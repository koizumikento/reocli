use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzDirection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    GetAbility {
        user_name: String,
    },
    GetDevInfo,
    GetUserAuth {
        user_name: String,
        password: String,
    },
    GetChannelStatus {
        channel: u8,
    },
    GetPtzStatus {
        channel: u8,
    },
    GetTime,
    SetTime {
        iso8601: String,
    },
    Snap {
        channel: u8,
        out: Option<String>,
    },
    PtzMove {
        channel: u8,
        direction: PtzDirection,
        speed: u8,
        duration_ms: Option<u64>,
    },
    PtzStop {
        channel: u8,
    },
    PtzPresetList {
        channel: u8,
    },
    PtzPresetGoto {
        channel: u8,
        preset_id: u8,
    },
    Preflight {
        user_name: String,
    },
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
        "get-channel-status" => Ok(CliCommand::GetChannelStatus {
            channel: parse_optional_channel(args.get(1), 0)?,
        }),
        "get-ptz-status" => Ok(CliCommand::GetPtzStatus {
            channel: parse_optional_channel(args.get(1), 0)?,
        }),
        "get-time" => Ok(CliCommand::GetTime),
        "set-time" => {
            let iso8601 = args.get(1).cloned().ok_or_else(|| {
                AppError::new(ErrorKind::InvalidInput, "set-time requires <iso8601>")
            })?;
            Ok(CliCommand::SetTime { iso8601 })
        }
        "snap" => parse_snap_args(&args[1..]),
        "ptz" => parse_ptz_args(&args[1..]),
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

fn parse_optional_channel(raw: Option<&String>, default_channel: u8) -> AppResult<u8> {
    match raw {
        Some(value) => parse_u8_arg(value, "channel"),
        None => Ok(default_channel),
    }
}

fn parse_snap_args(args: &[String]) -> AppResult<CliCommand> {
    let mut channel = 0;
    let mut channel_seen = false;
    let mut out = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(ErrorKind::InvalidInput, "snap --out requires <path>")
                })?;
                if value.trim().is_empty() {
                    return Err(AppError::new(
                        ErrorKind::InvalidInput,
                        "snap --out path must not be empty",
                    ));
                }
                out = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--") => {
                return Err(AppError::new(
                    ErrorKind::InvalidInput,
                    format!("unknown snap option: {flag}"),
                ));
            }
            raw_channel => {
                if channel_seen {
                    return Err(AppError::new(
                        ErrorKind::InvalidInput,
                        format!("unexpected snap argument: {raw_channel}"),
                    ));
                }
                channel = parse_u8_arg(raw_channel, "channel")?;
                channel_seen = true;
                index += 1;
            }
        }
    }

    Ok(CliCommand::Snap { channel, out })
}

fn parse_ptz_args(args: &[String]) -> AppResult<CliCommand> {
    let Some(action) = args.first() else {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ptz requires one of: move, stop, preset",
        ));
    };

    match action.as_str() {
        "move" => parse_ptz_move_args(&args[1..]),
        "stop" => parse_ptz_stop_args(&args[1..]),
        "preset" => parse_ptz_preset_args(&args[1..]),
        _ => Err(AppError::new(
            ErrorKind::InvalidInput,
            format!("unknown ptz action: {action}"),
        )),
    }
}

fn parse_ptz_move_args(args: &[String]) -> AppResult<CliCommand> {
    let Some(direction_raw) = args.first() else {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ptz move requires <direction>",
        ));
    };
    let direction = PtzDirection::parse(direction_raw).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("unknown PTZ direction: {direction_raw}"),
        )
    })?;

    let mut channel = 0u8;
    let mut speed = 32u8;
    let mut duration_ms = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--channel" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(ErrorKind::InvalidInput, "ptz move --channel requires <u8>")
                })?;
                channel = parse_u8_arg(value, "channel")?;
                index += 2;
            }
            "--speed" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(ErrorKind::InvalidInput, "ptz move --speed requires <u8>")
                })?;
                speed = parse_u8_arg(value, "speed")?;
                index += 2;
            }
            "--duration" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "ptz move --duration requires <milliseconds>",
                    )
                })?;
                duration_ms = Some(parse_u64_arg(value, "duration")?);
                index += 2;
            }
            unknown => {
                return Err(AppError::new(
                    ErrorKind::InvalidInput,
                    format!("unknown ptz move option: {unknown}"),
                ));
            }
        }
    }

    Ok(CliCommand::PtzMove {
        channel,
        direction,
        speed,
        duration_ms,
    })
}

fn parse_ptz_stop_args(args: &[String]) -> AppResult<CliCommand> {
    let channel = parse_ptz_channel_flag(args, "ptz stop")?;
    Ok(CliCommand::PtzStop { channel })
}

fn parse_ptz_preset_args(args: &[String]) -> AppResult<CliCommand> {
    let Some(action) = args.first() else {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ptz preset requires one of: list, goto",
        ));
    };

    match action.as_str() {
        "list" => {
            let channel = parse_ptz_channel_flag(&args[1..], "ptz preset list")?;
            Ok(CliCommand::PtzPresetList { channel })
        }
        "goto" => {
            let preset_raw = args.get(1).ok_or_else(|| {
                AppError::new(
                    ErrorKind::InvalidInput,
                    "ptz preset goto requires <preset_id>",
                )
            })?;
            let preset_id = parse_u8_arg(preset_raw, "preset_id")?;
            let channel = parse_ptz_channel_flag(&args[2..], "ptz preset goto")?;
            Ok(CliCommand::PtzPresetGoto { channel, preset_id })
        }
        _ => Err(AppError::new(
            ErrorKind::InvalidInput,
            format!("unknown ptz preset action: {action}"),
        )),
    }
}

fn parse_ptz_channel_flag(args: &[String], command_name: &str) -> AppResult<u8> {
    if args.is_empty() {
        return Ok(0);
    }

    if args.len() == 2 && args[0] == "--channel" {
        return parse_u8_arg(&args[1], "channel");
    }

    Err(AppError::new(
        ErrorKind::InvalidInput,
        format!("{command_name} only supports optional --channel <u8>"),
    ))
}

fn parse_u8_arg(raw: &str, name: &str) -> AppResult<u8> {
    raw.parse::<u8>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{name} must be an integer between 0 and 255"),
        )
    })
}

fn parse_u64_arg(raw: &str, name: &str) -> AppResult<u64> {
    raw.parse::<u64>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{name} must be a non-negative integer"),
        )
    })
}

pub fn help_text() -> &'static str {
    "Usage:\n  reocli help\n  reocli get-user-auth <user> <password>\n  reocli get-ability [user]\n  reocli get-dev-info\n  reocli get-channel-status [channel]\n  reocli get-ptz-status [channel]\n  reocli get-time\n  reocli set-time <iso8601>\n  reocli snap [channel] [--out path]\n  reocli ptz move <direction> [--speed <1-64>] [--duration <ms>] [--channel <0-255>]\n  reocli ptz stop [--channel <0-255>]\n  reocli ptz preset list [--channel <0-255>]\n  reocli ptz preset goto <preset_id> [--channel <0-255>]\n  reocli preflight [user]"
}
