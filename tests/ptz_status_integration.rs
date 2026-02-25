use mockito::{Matcher, Server};
use reocli::app::usecases;
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};

#[test]
fn parses_ptz_orientation_and_ranges() {
    let mut server = Server::new();

    let _cur_pos_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCurPos".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetPtzCurPos",
                "code":0,
                "value":{"PtzCurPos":{"channel":0,"Ppos":1870,"Tpos":-160}},
                "range":{"PtzCurPos":{"Ppos":{"min":-3550,"max":3550},"Tpos":{"min":-900,"max":900}}}
            }]"#,
        )
        .expect(1)
        .create();

    let _zoom_focus_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetZoomFocus".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetZoomFocus",
                "code":0,
                "value":{"ZoomFocus":{"channel":0,"focus":{"pos":130},"zoom":{"pos":21}}},
                "range":{"ZoomFocus":{"focus":{"pos":{"min":0,"max":223}},"zoom":{"pos":{"min":0,"max":33}}}}
            }]"#,
        )
        .expect(1)
        .create();

    let _preset_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetPtzPreset",
                "code":0,
                "range":{"PtzPreset":{"id":{"min":1,"max":64}}},
                "value":{"PtzPreset":[
                    {"channel":0,"enable":1,"id":0,"name":"zero"},
                    {"channel":0,"enable":1,"id":1,"name":"one"},
                    {"channel":0,"enable":0,"id":2,"name":"two"}
                ]}
            }]"#,
        )
        .expect(1)
        .create();

    let _check_state_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCheckState".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetPtzCheckState","code":0,"value":{"PtzCheckState":2}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let status = usecases::get_ptz_status::execute(&client, 0).expect("Get PTZ status failed");

    assert_eq!(status.channel, 0);
    assert_eq!(status.pan_position, Some(1870));
    assert_eq!(status.tilt_position, Some(-160));
    assert_eq!(status.zoom_position, Some(21));
    assert_eq!(status.focus_position, Some(130));
    assert_eq!(
        status.pan_range.as_ref().map(|range| range.min),
        Some(-3550)
    );
    assert_eq!(status.pan_range.as_ref().map(|range| range.max), Some(3550));
    assert_eq!(
        status.tilt_range.as_ref().map(|range| range.min),
        Some(-900)
    );
    assert_eq!(status.tilt_range.as_ref().map(|range| range.max), Some(900));
    assert_eq!(status.zoom_range.as_ref().map(|range| range.min), Some(0));
    assert_eq!(status.zoom_range.as_ref().map(|range| range.max), Some(33));
    assert_eq!(status.focus_range.as_ref().map(|range| range.min), Some(0));
    assert_eq!(
        status.focus_range.as_ref().map(|range| range.max),
        Some(223)
    );
    assert_eq!(status.preset_range.as_ref().map(|range| range.min), Some(1));
    assert_eq!(
        status.preset_range.as_ref().map(|range| range.max),
        Some(64)
    );
    assert_eq!(status.enabled_presets, vec![0, 1]);
    assert_eq!(status.calibration_state, Some(2));
    assert_eq!(status.calibrated(), Some(true));
}

#[test]
fn get_ptz_status_fails_when_all_ptz_commands_are_unsupported() {
    let mut server = Server::new();
    let mut mocks = Vec::new();

    for cmd in [
        "GetPtzCurPos",
        "GetZoomFocus",
        "GetPtzPreset",
        "GetPtzCheckState",
    ] {
        let mock = server
            .mock("POST", "/cgi-bin/api.cgi")
            .match_query(Matcher::UrlEncoded("cmd".to_string(), cmd.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"[{{"cmd":"{cmd}","code":1,"error":{{"detail":"unsupported"}}}}]"#
            ))
            .expect(1)
            .create();
        mocks.push(mock);
    }

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = usecases::get_ptz_status::execute(&client, 0).expect_err("expected PTZ error");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("PTZ status unavailable"));
}
