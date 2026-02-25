use std::process::Command;

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
}

#[test]
fn reocli_mcp_snap_works() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":0}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .arg("reolink.snap")
        .arg("1")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli-mcp reolink.snap");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = parse_stdout_json(&stdout);
    assert_eq!(json.get("channel").and_then(Value::as_u64), Some(1));
}

#[test]
fn reocli_mcp_get_user_auth_escapes_json() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetUserAuth".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetUserAuth","code":0,"value":{"Token":{"name":"tok\"en"}}}]"#)
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

fn parse_stdout_json(stdout: &str) -> Value {
    serde_json::from_str(stdout.trim()).expect("stdout should be valid JSON")
}
