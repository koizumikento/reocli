use crate::core::error::{AppError, AppResult, ErrorKind};
use crate::core::model::PtzDirection;

const DEFAULT_ABSOLUTE_RAW_TOL_COUNT: i64 = 10;
const DEFAULT_ABSOLUTE_TIMEOUT_MS: u64 = 25_000;

#[derive(Debug, Clone, PartialEq)]
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
    GetNetPort,
    SetTime {
        iso8601: String,
    },
    SetOnvif {
        enabled: bool,
        onvif_port: Option<u16>,
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
    PtzCalibrateAuto {
        channel: u8,
    },
    PtzSetAbsolute {
        channel: u8,
        pan_count: i64,
        tilt_count: i64,
        tol_count: i64,
        timeout_ms: u64,
    },
    PtzGetAbsolute {
        channel: u8,
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
        "get-net-port" => Ok(CliCommand::GetNetPort),
        "set-time" => {
            let iso8601 = args.get(1).cloned().ok_or_else(|| {
                AppError::new(ErrorKind::InvalidInput, "set-time requires <iso8601>")
            })?;
            Ok(CliCommand::SetTime { iso8601 })
        }
        "set-onvif" => parse_set_onvif_args(&args[1..]),
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

fn parse_set_onvif_args(args: &[String]) -> AppResult<CliCommand> {
    let enabled_raw = args.first().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "set-onvif requires <on|off> [--port <1-65535>]",
        )
    })?;
    let enabled = parse_bool_on_off(enabled_raw)?;

    let mut onvif_port = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--port" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "set-onvif --port requires <1-65535>",
                    )
                })?;
                onvif_port = Some(parse_u16_arg(value, "onvif_port")?);
                index += 2;
            }
            unknown => {
                return Err(AppError::new(
                    ErrorKind::InvalidInput,
                    format!("unknown set-onvif option: {unknown}"),
                ));
            }
        }
    }

    Ok(CliCommand::SetOnvif {
        enabled,
        onvif_port,
    })
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
            "ptz requires one of: move, stop, preset, calibrate, set-absolute, get-absolute",
        ));
    };

    match action.as_str() {
        "move" => parse_ptz_move_args(&args[1..]),
        "stop" => parse_ptz_stop_args(&args[1..]),
        "preset" => parse_ptz_preset_args(&args[1..]),
        "calibrate" => parse_ptz_calibrate_args(&args[1..]),
        "set-absolute" => parse_ptz_set_absolute_args(&args[1..]),
        "get-absolute" => parse_ptz_get_absolute_args(&args[1..]),
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

fn parse_ptz_calibrate_args(args: &[String]) -> AppResult<CliCommand> {
    let Some(action) = args.first() else {
        return Err(AppError::new(
            ErrorKind::InvalidInput,
            "ptz calibrate requires one of: auto",
        ));
    };

    match action.as_str() {
        "auto" => {
            let channel = parse_ptz_channel_flag(&args[1..], "ptz calibrate auto")?;
            Ok(CliCommand::PtzCalibrateAuto { channel })
        }
        _ => Err(AppError::new(
            ErrorKind::InvalidInput,
            format!("unknown ptz calibrate action: {action}"),
        )),
    }
}

fn parse_ptz_set_absolute_args(args: &[String]) -> AppResult<CliCommand> {
    let pan_raw = args.first().ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "ptz set-absolute requires <pan_count> <tilt_count>",
        )
    })?;
    let tilt_raw = args.get(1).ok_or_else(|| {
        AppError::new(
            ErrorKind::InvalidInput,
            "ptz set-absolute requires <pan_count> <tilt_count>",
        )
    })?;
    let pan_count = parse_i64_arg(pan_raw, "pan_count")?;
    let tilt_count = parse_i64_arg(tilt_raw, "tilt_count")?;

    let mut channel = 0u8;
    let mut tol_count = DEFAULT_ABSOLUTE_RAW_TOL_COUNT;
    let mut timeout_ms = DEFAULT_ABSOLUTE_TIMEOUT_MS;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--channel" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "ptz set-absolute --channel requires <u8>",
                    )
                })?;
                channel = parse_u8_arg(value, "channel")?;
                index += 2;
            }
            "--tol-count" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "ptz set-absolute --tol-count requires <i64>",
                    )
                })?;
                tol_count = parse_i64_arg(value, "tol_count")?;
                index += 2;
            }
            "--timeout-ms" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new(
                        ErrorKind::InvalidInput,
                        "ptz set-absolute --timeout-ms requires <u64>",
                    )
                })?;
                timeout_ms = parse_u64_arg(value, "timeout_ms")?;
                index += 2;
            }
            unknown => {
                return Err(AppError::new(
                    ErrorKind::InvalidInput,
                    format!("unknown ptz set-absolute option: {unknown}"),
                ));
            }
        }
    }

    Ok(CliCommand::PtzSetAbsolute {
        channel,
        pan_count,
        tilt_count,
        tol_count,
        timeout_ms,
    })
}

fn parse_ptz_get_absolute_args(args: &[String]) -> AppResult<CliCommand> {
    let channel = parse_ptz_channel_flag(args, "ptz get-absolute")?;
    Ok(CliCommand::PtzGetAbsolute { channel })
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

fn parse_u16_arg(raw: &str, name: &str) -> AppResult<u16> {
    raw.parse::<u16>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{name} must be an integer between 0 and 65535"),
        )
    })
}

fn parse_bool_on_off(raw: &str) -> AppResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "enable" | "enabled" => Ok(true),
        "0" | "false" | "off" | "disable" | "disabled" => Ok(false),
        _ => Err(AppError::new(
            ErrorKind::InvalidInput,
            "set-onvif requires <on|off>",
        )),
    }
}

fn parse_i64_arg(raw: &str, name: &str) -> AppResult<i64> {
    raw.parse::<i64>().map_err(|_| {
        AppError::new(
            ErrorKind::InvalidInput,
            format!("{name} must be an integer"),
        )
    })
}

pub fn help_text() -> &'static str {
    "Usage:\n  reocli help\n  reocli get-user-auth <user> <password>\n  reocli get-ability [user]\n  reocli get-dev-info\n  reocli get-channel-status [channel]\n  reocli get-ptz-status [channel]\n  reocli get-time\n  reocli get-net-port\n  reocli set-time <iso8601>\n  reocli set-onvif <on|off> [--port <1-65535>]\n  reocli snap [channel] [--out path]\n  reocli ptz move <direction> [--speed <1-64>] [--duration <ms>] [--channel <0-255>]\n  reocli ptz stop [--channel <0-255>]\n  reocli ptz preset list [--channel <0-255>]\n  reocli ptz preset goto <preset_id> [--channel <0-255>]\n  reocli ptz calibrate auto [--channel <0-255>]\n  reocli ptz set-absolute <pan_count> <tilt_count> [--tol-count <i64>] [--timeout-ms <u64>] [--channel <0-255>]\n  reocli ptz get-absolute [--channel <0-255>]\n  reocli preflight [user]"
}
