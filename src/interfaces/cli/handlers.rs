use crate::app::preflight::run_preflight;
use crate::app::usecases;
use crate::core::error::AppResult;
use crate::interfaces::cli::args::{CliCommand, help_text, parse_args};
use crate::reolink::client::{Auth, Client};

const DEFAULT_ENDPOINT: &str = "https://camera.local";

pub fn run(args: &[String]) -> AppResult<String> {
    let command = parse_args(args)?;
    let client = Client::new(endpoint_from_env(), Auth::Anonymous);

    match command {
        CliCommand::Help => Ok(help_text().to_string()),
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

fn endpoint_from_env() -> String {
    std::env::var("REOCLI_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}
