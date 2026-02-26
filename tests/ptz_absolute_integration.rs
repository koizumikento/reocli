use mockito::{Matcher, Server};
use reocli::app::usecases::ptz_set_absolute_raw;
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};

#[test]
fn set_absolute_success_path_reaches_target() {
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
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":0,"Ppos":1500,"Tpos":-180}}}]"#,
        )
        .expect(1)
        .create();
    let _stop_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::Regex(r#""op":"Stop""#.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let result =
        ptz_set_absolute_raw::execute(&client, 0, 1500, -180, 5, 200).expect("set_absolute failed");

    assert_eq!(result.channel, 0);
    assert_eq!(result.pan_count, 1500);
    assert_eq!(result.tilt_count, -180);
}

#[test]
fn set_absolute_timeout_path_returns_error() {
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
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":0,"Ppos":0,"Tpos":0}}}]"#,
        )
        .expect_at_least(1)
        .expect_at_most(10)
        .create();
    let _move_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::Regex(
            r#""op":"(Left|Right|Up|Down|LeftUp|LeftDown|RightUp|RightDown)""#.to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect_at_least(1)
        .expect_at_most(16)
        .create();
    let _stop_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::Regex(r#""op":"Stop""#.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect_at_least(1)
        .expect_at_most(16)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = ptz_set_absolute_raw::execute(&client, 0, 1500, -180, 5, 80)
        .expect_err("set_absolute should timeout");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("timeout"));
}
