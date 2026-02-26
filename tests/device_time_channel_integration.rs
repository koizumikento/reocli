use mockito::{Matcher, Server};
use reocli::app::usecases;
use reocli::core::command::CgiCommand;
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};

#[test]
fn parses_device_time_and_channel_responses() {
    let mut server = Server::new();

    let _ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetAbility".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetAbility",
                "code":0,
                "value":{
                    "Ability":{
                        "GetDevInfo":{"permit":1},
                        "GetChannelStatus":{"permit":1},
                        "GetTime":{"permit":1},
                        "SetTime":{"permit":1},
                        "Snap":{"permit":0},
                        "GetUserAuth":{"permit":0},
                        "UnknownCommand":{"permit":1}
                    }
                }
            }]"#,
        )
        .expect(1)
        .create();

    let _dev_info_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetDevInfo",
                "code":0,
                "value":{
                    "DevInfo":{
                        "model":"RLC-811A",
                        "firmVer":"v3.1.0",
                        "serial":"RL-12345678"
                    }
                }
            }]"#,
        )
        .expect(1)
        .create();

    let _channel_status_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetChannelStatus".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetChannelStatus",
                "code":0,
                "value":{
                    "channels":[
                        {"channel":0,"online":0},
                        {"channel":1,"online":1}
                    ]
                }
            }]"#,
        )
        .expect(1)
        .create();

    let _get_time_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetTime".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetTime",
                "code":0,
                "value":{
                    "Time":{"localTime":"2026-02-25T09:10:11Z"}
                }
            }]"#,
        )
        .expect(1)
        .create();

    let _set_time_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "SetTime".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"SetTime","code":0,"value":{"rspCode":200}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);

    let ability = usecases::get_ability::execute(&client, "admin").expect("GetAbility failed");
    assert!(ability.supports(CgiCommand::GetDevInfo));
    assert!(ability.supports(CgiCommand::GetChannelStatus));
    assert!(ability.supports(CgiCommand::GetTime));
    assert!(ability.supports(CgiCommand::SetTime));
    assert!(!ability.supports(CgiCommand::Snap));
    assert!(!ability.supports(CgiCommand::GetUserAuth));

    let dev_info = usecases::get_dev_info::execute(&client).expect("GetDevInfo failed");
    assert_eq!(dev_info.model, "RLC-811A");
    assert_eq!(dev_info.firmware, "v3.1.0");
    assert_eq!(dev_info.serial_number, "RL-12345678");

    let channel_status =
        usecases::get_channel_status::execute(&client, 1).expect("GetChannelStatus failed");
    assert_eq!(channel_status.channel, 1);
    assert!(channel_status.online);

    let current_time = usecases::get_time::execute(&client).expect("GetTime failed");
    assert_eq!(current_time.iso8601, "2026-02-25T09:10:11Z");

    let target_time = "2026-02-25T10:00:00Z";
    let updated_time = usecases::set_time::execute(&client, target_time).expect("SetTime failed");
    assert_eq!(updated_time.iso8601, target_time);
}

#[test]
fn get_channel_status_fails_when_requested_channel_is_missing() {
    let mut server = Server::new();
    let _channel_status_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetChannelStatus".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{
                "cmd":"GetChannelStatus",
                "code":0,
                "value":{
                    "channels":[
                        {"channel":0,"online":1}
                    ]
                }
            }]"#,
        )
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = usecases::get_channel_status::execute(&client, 1).expect_err("expected error");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("missing online state"));
}

#[test]
fn set_time_rejects_non_rfc3339_like_timestamp() {
    let client = Client::new("http://camera.local", Auth::Anonymous);

    for invalid_time in [
        "2026-02-25 10:00:00Z",
        "2026-13-25T10:00:00Z",
        "2026-02-25T10:00:00",
        "not-a-time",
    ] {
        let error = usecases::set_time::execute(&client, invalid_time)
            .expect_err("invalid timestamp must be rejected before request execution");
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert!(error.message.contains("RFC3339-like"));
    }
}
