use mockito::{Matcher, Server};
use reocli::app::usecases;
use reocli::core::error::ErrorKind;
use reocli::core::model::PtzDirection;
use reocli::reolink::client::{Auth, Client};

#[test]
fn ptz_move_with_duration_sends_move_then_stop() {
    let mut server = Server::new();

    let _move_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":1"#.to_string()),
            Matcher::Regex(r#""op":"Left""#.to_string()),
            Matcher::Regex(r#""speed":8"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let _stop_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":1"#.to_string()),
            Matcher::Regex(r#""op":"Stop""#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    usecases::ptz_move::execute(&client, 1, PtzDirection::Left, 8, Some(0))
        .expect("ptz move with duration should succeed");
}

#[test]
fn ptz_move_rejects_speed_out_of_range() {
    let client = Client::new("http://localhost".to_string(), Auth::Anonymous);

    let error = usecases::ptz_move::execute(&client, 0, PtzDirection::Up, 0, None)
        .expect_err("speed=0 must fail");
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(error.message.contains("1..=64"));

    let error = usecases::ptz_move::execute(&client, 0, PtzDirection::Up, 65, None)
        .expect_err("speed=65 must fail");
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(error.message.contains("1..=64"));
}

#[test]
fn ptz_move_returns_error_when_device_returns_non_zero_code() {
    let mut server = Server::new();

    let _move_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":0"#.to_string()),
            Matcher::Regex(r#""op":"Right""#.to_string()),
            Matcher::Regex(r#""speed":9"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":1}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = usecases::ptz_move::execute(&client, 0, PtzDirection::Right, 9, None)
        .expect_err("expected non-zero code failure");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("PtzCtrl failed with code=1"));
}

#[test]
fn ptz_stop_sends_stop_operation() {
    let mut server = Server::new();

    let _stop_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":2"#.to_string()),
            Matcher::Regex(r#""op":"Stop""#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    usecases::ptz_stop::execute(&client, 2).expect("ptz stop should succeed");
}

#[test]
fn ptz_preset_list_requests_get_ptz_preset_and_parses_enabled_presets() {
    let mut server = Server::new();

    let _preset_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .match_body(Matcher::Regex(r#""channel":3"#.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzPreset","code":0,"value":{"PtzPreset":[
                {"channel":3,"enable":1,"id":1,"name":"Home"},
                {"channel":3,"enable":0,"id":2,"name":"Disabled"},
                {"channel":4,"enable":1,"id":3,"name":"OtherChannel"},
                {"channel":3,"enable":1,"id":255,"name":"Far"},
                {"channel":3,"enable":1,"id":0,"name":"InvalidMin"},
                {"channel":3,"enable":1,"id":256,"name":"InvalidMax"}
            ]}}]"#,
        )
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let presets =
        usecases::ptz_preset_list::execute(&client, 3).expect("preset listing should succeed");

    assert_eq!(presets.len(), 2);
    assert_eq!(presets[0].id.value(), 1);
    assert_eq!(presets[0].name.as_deref(), Some("Home"));
    assert_eq!(presets[1].id.value(), 255);
    assert_eq!(presets[1].name.as_deref(), Some("Far"));
}

#[test]
fn ptz_preset_list_returns_error_when_device_returns_non_zero_code() {
    let mut server = Server::new();

    let _preset_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .match_body(Matcher::Regex(r#""channel":3"#.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetPtzPreset","code":1}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error =
        usecases::ptz_preset_list::execute(&client, 3).expect_err("expected preset list failure");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("GetPtzPreset failed with code=1"));
}

#[test]
fn ptz_preset_goto_sends_to_pos_operation() {
    let mut server = Server::new();

    let _goto_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":0"#.to_string()),
            Matcher::Regex(r#""op":"ToPos""#.to_string()),
            Matcher::Regex(r#""id":7"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    usecases::ptz_preset_goto::execute(&client, 0, 7).expect("preset goto should succeed");
}

#[test]
fn ptz_preset_goto_rejects_invalid_preset_id() {
    let client = Client::new("http://localhost".to_string(), Auth::Anonymous);

    let error =
        usecases::ptz_preset_goto::execute(&client, 0, 0).expect_err("preset id 0 must fail");

    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(error.message.contains("1..=255"));
}
