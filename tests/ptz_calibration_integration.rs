use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use mockito::{Matcher, Server};
use reocli::app::usecases::ptz_calibrate_auto;
use reocli::reolink::client::{Auth, Client};

#[test]
fn calibrate_auto_saves_and_reuses_saved_params_for_same_camera_key() {
    let unique = unique_suffix();
    let serial = format!("SERIAL-{unique}");
    let model = format!("Model-{unique}");
    let firmware = format!("v1.{unique}");

    let mut server = Server::new();
    let _dev_info_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{
                "cmd":"GetDevInfo",
                "code":0,
                "value":{{"DevInfo":{{"model":"{model}","firmware":"{firmware}","serial":"{serial}"}}}}
            }}]"#
        ))
        .expect(2)
        .create();

    let _cur_pos_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCurPos".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzCurPos","code":0,
                "value":{"PtzCurPos":{"channel":0,"Ppos":120,"Tpos":40}},
                "range":{"PtzCurPos":{"Ppos":{"min":-3550,"max":3550},"Tpos":{"min":0,"max":900}}}
            }]"#,
        )
        .expect(2)
        .create();

    let _zoom_focus_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetZoomFocus".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetZoomFocus","code":1}]"#)
        .expect(2)
        .create();

    let _preset_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetPtzPreset","code":1}]"#)
        .expect(2)
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
        .expect(2)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let first = ptz_calibrate_auto::execute(&client, 0).expect("first calibration should succeed");
    let second =
        ptz_calibrate_auto::execute(&client, 0).expect("second calibration should reuse file");

    assert!(!first.reused_existing);
    assert!(second.reused_existing);
    assert_eq!(first.camera_key, second.camera_key);
    assert_eq!(first.params, second.params);
    assert!(Path::new(&first.calibration_path).exists());

    cleanup_file(&first.calibration_path);
}

fn unique_suffix() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{now}-{}", std::process::id())
}

fn cleanup_file(path: &str) {
    let path = Path::new(path);
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}
