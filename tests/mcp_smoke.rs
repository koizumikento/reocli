use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use mockito::{Matcher, Server};
use serde_json::Value;

#[test]
fn reocli_mcp_lists_tools() {
    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .output()
        .expect("failed to run reocli-mcp");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = parse_stdout_json(&stdout);
    let tools = json
        .get("tools")
        .and_then(Value::as_array)
        .expect("tools should be an array");
    assert!(tools.contains(&Value::String("mcp.list_tools".to_string())));
    assert!(tools.contains(&Value::String("reolink.ptz_calibrate_auto".to_string())));
    assert!(tools.contains(&Value::String("reolink.ptz_set_absolute".to_string())));
    assert!(tools.contains(&Value::String("reolink.ptz_get_absolute".to_string())));
}

#[test]
fn reocli_mcp_snap_works() {
    let mut server = Server::new();
    let _ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetAbility".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"Snap":{"permit":1}}}}]"#)
        .expect(1)
        .create();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":0,"value":{"base64":"aGVsbG8="}}]"#)
        .expect(1)
        .create();

    let output_path = unique_temp_file_path("mcp-snap", "channel-1.jpg");
    cleanup_output_file(&output_path);
    let output_string = output_path.to_string_lossy().into_owned();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.snap")
        .arg("1")
        .arg(&output_string)
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli-mcp reolink.snap");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = parse_stdout_json(&stdout);
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(1));
    assert_eq!(
        json.get("image_path").and_then(Value::as_str),
        Some(output_string.as_str())
    );
    assert_eq!(json.get("bytes_written").and_then(Value::as_u64), Some(5));
    assert_eq!(
        fs::read(&output_path).expect("snapshot output should exist"),
        b"hello"
    );
    cleanup_output_file(&output_path);
}

#[test]
fn reocli_mcp_get_user_auth_escapes_json() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Login".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Login","code":0,"value":{"Token":{"name":"tok\"en"}}}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.get_user_auth")
        .arg("admin")
        .arg("secret")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli-mcp reolink.get_user_auth");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = parse_stdout_json(&stdout);
    assert_eq!(json.get("token").and_then(Value::as_str), Some("tok\"en"));
}

#[test]
fn reocli_mcp_get_and_set_time_work() {
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
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetTime":{"permit":1},"SetTime":{"permit":1}}}}]"#,
        )
        .expect(2)
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
            r#"[{"cmd":"GetTime","code":0,"value":{"Time":{"localTime":"2026-02-25T09:10:11Z"}}}]"#,
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
        .with_body(r#"[{"cmd":"SetTime","code":0}]"#)
        .expect(1)
        .create();

    let get_time_output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.get_time")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli-mcp reolink.get_time");
    assert!(get_time_output.status.success());
    let get_time_json = parse_stdout_json(&String::from_utf8_lossy(&get_time_output.stdout));
    assert_eq!(
        get_time_json.get("time").and_then(Value::as_str),
        Some("2026-02-25T09:10:11Z")
    );

    let target_time = "2026-02-25T10:00:00Z";
    let set_time_output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.set_time")
        .arg(target_time)
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli-mcp reolink.set_time");
    assert!(set_time_output.status.success());
    let set_time_json = parse_stdout_json(&String::from_utf8_lossy(&set_time_output.stdout));
    assert_eq!(
        set_time_json.get("time").and_then(Value::as_str),
        Some(target_time)
    );
}

#[test]
fn reocli_mcp_get_ptz_status_works() {
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
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":0,"Ppos":900,"Tpos":-120}}}]"#,
        )
        .expect(1)
        .create();

    let _zoom_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetZoomFocus".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetZoomFocus","code":1}]"#)
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
        .with_body(r#"[{"cmd":"GetPtzPreset","code":1}]"#)
        .expect(1)
        .create();

    let _check_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCheckState".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetPtzCheckState","code":1}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.get_ptz_status")
        .arg("0")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli-mcp reolink.get_ptz_status");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = parse_stdout_json(&stdout);
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(0));
    assert_eq!(json.get("pan").and_then(Value::as_i64), Some(900));
    assert_eq!(json.get("tilt").and_then(Value::as_i64), Some(-120));
    assert!(json.get("pan_deg").is_none());
    assert!(json.get("tilt_deg").is_none());
}

#[test]
fn reocli_mcp_ptz_move_works() {
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
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"PtzCtrl":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _move_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":2"#.to_string()),
            Matcher::Regex(r#""op":"Left""#.to_string()),
            Matcher::Regex(r#""speed":6"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.ptz_move")
        .arg("2")
        .arg("left")
        .arg("6")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reolink.ptz_move");

    assert!(output.status.success());
    let json = parse_stdout_json(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(json.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(2));
}

#[test]
fn reocli_mcp_ptz_preset_list_and_goto_work() {
    let mut server = Server::new();
    let _list_ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetAbility".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetPtzPreset":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _list_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzPreset","code":0,"value":{"PtzPreset":[{"channel":3,"enable":1,"id":7,"name":"Home"}]}}]"#,
        )
        .expect(1)
        .create();

    let list_output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.ptz_preset_list")
        .arg("3")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reolink.ptz_preset_list");
    assert!(list_output.status.success());
    let list_json = parse_stdout_json(&String::from_utf8_lossy(&list_output.stdout));
    assert_eq!(list_json.get("channel").and_then(Value::as_u64), Some(3));
    assert_eq!(
        list_json
            .get("presets")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_u64),
        Some(7)
    );

    let _goto_ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetAbility".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"PtzCtrl":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _goto_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""channel":3"#.to_string()),
            Matcher::Regex(r#""op":"ToPos""#.to_string()),
            Matcher::Regex(r#""id":7"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let goto_output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.ptz_preset_goto")
        .arg("3")
        .arg("7")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reolink.ptz_preset_goto");
    assert!(goto_output.status.success());
    let goto_json = parse_stdout_json(&String::from_utf8_lossy(&goto_output.stdout));
    assert_eq!(goto_json.get("ok").and_then(Value::as_bool), Some(true));
}

#[test]
fn reocli_mcp_ptz_calibrate_auto_works() {
    let mut server = Server::new();
    setup_ptz_absolute_mocks(&mut server);
    let calibration_dir = unique_temp_dir("mcp-ptz-calibrate");
    cleanup_output_dir(&calibration_dir);

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.ptz_calibrate_auto")
        .arg("0")
        .env("REOCLI_ENDPOINT", server.url())
        .env("REOCLI_CALIBRATION_DIR", &calibration_dir)
        .output()
        .expect("failed to run reolink.ptz_calibrate_auto");

    assert!(output.status.success());
    let json = parse_stdout_json(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(0));
    assert!(json.get("camera_key").is_some());
    assert!(json.get("pan_count").and_then(Value::as_i64).is_some());
    assert!(json.get("tilt_count").and_then(Value::as_i64).is_some());
    let calibration = json.get("calibration").expect("calibration should exist");
    assert!(calibration.get("pan_min_count").is_some());
    assert!(calibration.get("pan_max_count").is_some());
    assert!(calibration.get("pan_deadband_count").is_some());
    assert!(calibration.get("tilt_min_count").is_some());
    assert!(calibration.get("tilt_max_count").is_some());
    assert!(calibration.get("tilt_deadband_count").is_some());
    assert!(calibration.get("pan_offset").is_none());
    assert!(calibration.get("pan_scale").is_none());
    assert!(calibration.get("tilt_offset").is_none());
    assert!(calibration.get("tilt_scale").is_none());
    let report = json.get("report").expect("report should exist");
    assert!(report.get("pan_error_p95_count").is_some());
    assert!(report.get("tilt_error_p95_count").is_some());
    assert!(report.get("pan_error_p95_deg").is_none());
    assert!(report.get("tilt_error_p95_deg").is_none());
    cleanup_output_dir(&calibration_dir);
}

#[test]
fn reocli_mcp_ptz_set_absolute_works() {
    let mut server = Server::new();
    setup_ptz_absolute_mocks(&mut server);

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.ptz_set_absolute")
        .arg("0")
        .arg("1500")
        .arg("-180")
        .arg("5")
        .arg("4000")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reolink.ptz_set_absolute");

    assert!(output.status.success());
    let json = parse_stdout_json(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(0));
    assert_eq!(json.get("pan_count").and_then(Value::as_i64), Some(1500));
    assert_eq!(json.get("tilt_count").and_then(Value::as_i64), Some(-180));
}

#[test]
fn reocli_mcp_ptz_get_absolute_works() {
    let mut server = Server::new();
    setup_ptz_absolute_mocks(&mut server);

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.ptz_get_absolute")
        .arg("0")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reolink.ptz_get_absolute");

    assert!(output.status.success());
    let json = parse_stdout_json(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(0));
    assert_eq!(json.get("pan_count").and_then(Value::as_i64), Some(1200));
    assert_eq!(json.get("tilt_count").and_then(Value::as_i64), Some(-80));
}

fn parse_stdout_json(stdout: &str) -> Value {
    serde_json::from_str(stdout.trim()).expect("stdout should be valid JSON")
}

fn unique_temp_file_path(prefix: &str, file_name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("reocli-{prefix}-{now}-{}", std::process::id()))
        .join(file_name)
}

fn cleanup_output_file(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir_all(parent);
    }
}

fn setup_ptz_absolute_mocks(server: &mut Server) {
    let _ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetAbility".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetDevInfo":{"permit":1},"GetPtzCurPos":{"permit":1},"GetPtzCheckState":{"permit":1},"PtzCtrl":{"permit":1}}}}]"#,
        )
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
            r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"name":"RLC-823A","firmVer":"v3.0.0","serial":"ABC123"}}}]"#,
        )
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
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":0,"Ppos":1200,"Tpos":-80}}}]"#,
        )
        .expect(1)
        .create();
    let _cur_pos_after_move_mock = server
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
        .create();
    let _check_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCheckState".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzCheckState","code":0,"value":{"PtzCheckState":{"channel":0,"state":2}}}]"#,
        )
        .create();
    let _ptz_ctrl_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .create();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("reocli-{prefix}-{now}-{}", std::process::id()))
}

fn cleanup_output_dir(path: &Path) {
    if path.exists() {
        let _ = fs::remove_dir_all(path);
    }
}
