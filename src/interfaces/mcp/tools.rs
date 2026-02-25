#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpTool {
    pub name: &'static str,
    pub description: &'static str,
}

pub fn supported_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "mcp.list_tools",
            description: "List supported MCP tools.",
        },
        McpTool {
            name: "reolink.get_ability",
            description: "Get command availability for the current camera model.",
        },
        McpTool {
            name: "reolink.get_user_auth",
            description: "Exchange user/password for API token.",
        },
        McpTool {
            name: "reolink.get_dev_info",
            description: "Fetch camera model and firmware information.",
        },
        McpTool {
            name: "reolink.get_channel_status",
            description: "Get online/offline status for a channel.",
        },
        McpTool {
            name: "reolink.get_time",
            description: "Get camera time.",
        },
        McpTool {
            name: "reolink.set_time",
            description: "Set camera time.",
        },
        McpTool {
            name: "reolink.snap",
            description: "Capture a snapshot for a channel.",
        },
    ]
}
