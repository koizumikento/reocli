use crate::app::preflight::run_preflight;
use crate::app::usecases;
use crate::core::error::AppResult;
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
