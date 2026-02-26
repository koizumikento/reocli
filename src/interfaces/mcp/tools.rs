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
            name: "reolink.get_ptz_status",
            description: "Get PTZ raw positions/ranges for a channel and calibrated degrees when available.",
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
            description: "Capture a snapshot for a channel and save it.",
        },
        McpTool {
            name: "reolink.ptz_move",
            description: "Move PTZ direction with speed and optional duration.",
        },
        McpTool {
            name: "reolink.ptz_stop",
            description: "Stop PTZ movement immediately.",
        },
        McpTool {
            name: "reolink.ptz_preset_list",
            description: "List enabled PTZ presets.",
        },
        McpTool {
            name: "reolink.ptz_preset_goto",
            description: "Move PTZ to a preset ID.",
        },
        McpTool {
            name: "reolink.ptz_calibrate_auto",
            description: "Run PTZ auto calibration and return calibration/report summary.",
        },
        McpTool {
            name: "reolink.ptz_set_absolute",
            description: "Move PTZ to an absolute pan/tilt target.",
        },
        McpTool {
            name: "reolink.ptz_get_absolute",
            description: "Get current PTZ absolute pan/tilt position.",
        },
    ]
}
