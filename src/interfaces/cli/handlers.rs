use crate::app::preflight::run_preflight;
use crate::app::usecases;
use crate::core::error::AppResult;
use crate::core::model::NumericRange;
use crate::interfaces::cli::args::{CliCommand, help_text, parse_args};
use crate::interfaces::runtime::client_from_env;

pub fn run(args: &[String]) -> AppResult<String> {
    let command = parse_args(args)?;
    let client = client_from_env();

    match command {
        CliCommand::Help => Ok(help_text().to_string()),
        CliCommand::GetUserAuth {
            user_name,
            password,
        } => {
            let token = usecases::get_user_auth::execute(&client, &user_name, &password)?;
            Ok(format!("token={token}"))
        }
        CliCommand::GetAbility { user_name } => {
            let ability = usecases::get_ability::execute(&client, &user_name)?;
            let commands = ability
                .supported_commands
                .iter()
                .map(|command| command.as_str())
                .collect::<Vec<_>>()
                .join(",");
            Ok(format!(
                "user={}; supported=[{}]",
                ability.user_name, commands
            ))
        }
        CliCommand::GetDevInfo => {
            let info = usecases::get_dev_info::execute(&client)?;
            Ok(format!(
                "model={}; firmware={}; serial={}",
                info.model, info.firmware, info.serial_number
            ))
        }
        CliCommand::GetChannelStatus { channel } => {
            let status = usecases::get_channel_status::execute(&client, channel)?;
            Ok(format!(
                "channel={}; online={}",
                status.channel, status.online
            ))
        }
        CliCommand::GetPtzStatus { channel } => {
            let status = usecases::get_ptz_status::execute(&client, channel)?;
            let presets = if status.enabled_presets.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "[{}]",
                    status
                        .enabled_presets
                        .iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            Ok(format!(
                "channel={}; pan={}; tilt={}; zoom={}; focus={}; pan_range={}; tilt_range={}; zoom_range={}; focus_range={}; preset_range={}; enabled_presets={}; calibration_state={}; calibrated={}",
                status.channel,
                format_optional_i64(status.pan_position),
                format_optional_i64(status.tilt_position),
                format_optional_i64(status.zoom_position),
                format_optional_i64(status.focus_position),
                format_optional_range(status.pan_range.as_ref()),
                format_optional_range(status.tilt_range.as_ref()),
                format_optional_range(status.zoom_range.as_ref()),
                format_optional_range(status.focus_range.as_ref()),
                format_optional_range(status.preset_range.as_ref()),
                presets,
                format_optional_i64(status.calibration_state),
                format_optional_bool(status.calibrated()),
            ))
        }
        CliCommand::GetTime => {
            let time = usecases::get_time::execute(&client)?;
            Ok(format!("time={}", time.iso8601))
        }
        CliCommand::SetTime { iso8601 } => {
            let time = usecases::set_time::execute(&client, &iso8601)?;
            Ok(format!("time={}", time.iso8601))
        }
        CliCommand::Snap { channel } => {
            let snapshot = usecases::snap::execute(&client, channel)?;
            Ok(format!(
                "channel={}; image_path={}",
                snapshot.channel, snapshot.image_path
            ))
        }
        CliCommand::Preflight { user_name } => {
            let report = run_preflight(&client, &user_name)?;
            Ok(format!(
                "model={}; supported_commands={}",
                report.device_info.model,
                report.supported_commands.len()
            ))
        }
    }
}

fn format_optional_i64(value: Option<i64>) -> String {
    value
        .map(|raw| raw.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_optional_bool(value: Option<bool>) -> String {
    value
        .map(|flag| flag.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_optional_range(range: Option<&NumericRange>) -> String {
    range
        .map(|bounds| format!("{}..{}", bounds.min, bounds.max))
        .unwrap_or_else(|| "unknown".to_string())
}
