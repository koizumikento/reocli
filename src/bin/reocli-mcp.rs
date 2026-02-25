use std::process::ExitCode;

use reocli::interfaces::mcp::handlers::{McpRequest, handle_request};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let tool = args.next().unwrap_or_else(|| "mcp.list_tools".to_string());
    let arguments = args.collect::<Vec<_>>();

    match handle_request(McpRequest { tool, arguments }) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
