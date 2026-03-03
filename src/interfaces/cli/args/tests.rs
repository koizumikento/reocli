use super::*;

fn parse(raw: &[&str]) -> AppResult<CliCommand> {
    parse_args(
        &raw.iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
    )
}

#[test]
fn parse_ptz_onvif_status_with_default_channel() {
    let command = parse(&["ptz", "onvif", "status"]).expect("status command should parse");
    assert_eq!(command, CliCommand::PtzOnvifStatus { channel: 0 });
}

#[test]
fn parse_ptz_onvif_options_with_channel_flag() {
    let command = parse(&["ptz", "onvif", "options", "--channel", "7"])
        .expect("options command should parse");
    assert_eq!(command, CliCommand::PtzOnvifOptions { channel: 7 });
}

#[test]
fn parse_ptz_onvif_relative_move_with_channel_flag() {
    let command = parse(&[
        "ptz",
        "onvif",
        "relative-move",
        "55",
        "-12",
        "--channel",
        "3",
    ])
    .expect("relative-move command should parse");
    assert_eq!(
        command,
        CliCommand::PtzOnvifRelativeMove {
            channel: 3,
            pan_delta_count: 55,
            tilt_delta_count: -12,
        }
    );
}

#[test]
fn parse_ptz_onvif_relative_move_requires_two_deltas() {
    let error = parse(&["ptz", "onvif", "relative-move", "55"])
        .expect_err("relative-move should fail without tilt delta");
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(
        error
            .message
            .contains("requires <pan_delta_count> <tilt_delta_count>")
    );
}
