use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use mockito::{Matcher, Server};

#[test]
fn reocli_help_works() {
    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("help")
        .output()
        .expect("failed to run reocli help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
}

#[test]
fn reocli_get_dev_info_works() {
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
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetDevInfo":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"name":"cam"}}}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-dev-info")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli get-dev-info");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("model="));
}

#[test]
fn reocli_get_net_port_works() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetNetPort".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetNetPort","code":0,"value":{"NetPort":{"httpEnable":0,"httpPort":80,"httpsEnable":1,"httpsPort":443,"mediaPort":9000,"onvifEnable":1,"onvifPort":8000}}}]"#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-net-port")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli get-net-port");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("onvif_enable=true"));
    assert!(stdout.contains("onvif_port=8000"));
}

#[test]
fn reocli_set_onvif_works() {
    let mut server = Server::new();
    let _get_before_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetNetPort".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetNetPort","code":0,"value":{"NetPort":{"onvifEnable":0,"onvifPort":8000}}}]"#,
        )
        .expect(1)
        .create();
    let _set_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "SetNetPort".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""action":1"#.to_string()),
            Matcher::Regex(r#""onvifEnable":1"#.to_string()),
            Matcher::Regex(r#""onvifPort":8000"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"SetNetPort","code":0,"value":{"rspCode":200}}]"#)
        .expect(1)
        .create();
    let _get_after_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetNetPort".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetNetPort","code":0,"value":{"NetPort":{"onvifEnable":1,"onvifPort":8000}}}]"#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("set-onvif")
        .arg("on")
        .arg("--port")
        .arg("8000")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli set-onvif");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=set_onvif"));
    assert!(stdout.contains("onvif_enable=true"));
    assert!(stdout.contains("onvif_port=8000"));
}

#[test]
fn reocli_uses_admin_when_only_password_env_is_set() {
    let mut server = Server::new();
    let _login_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Login".to_string()))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""userName":"admin""#.to_string()),
            Matcher::Regex(r#""password":"secret""#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Login","code":0,"value":{"Token":{"name":"issued-token"}}}]"#)
        .expect(2)
        .create();

    let _ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetAbility".to_string()),
            Matcher::UrlEncoded("token".to_string(), "issued-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetDevInfo":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();

    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetDevInfo".to_string()),
            Matcher::UrlEncoded("token".to_string(), "issued-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"name":"cam"}}}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-dev-info")
        .env("REOCLI_ENDPOINT", server.url())
        .env("REOCLI_PASSWORD", "secret")
        .env_remove("REOCLI_USER")
        .env_remove("REOCLI_TOKEN")
        .output()
        .expect("failed to run reocli get-dev-info with default admin user");

    assert!(output.status.success());
}

#[test]
fn reocli_uses_token_cache_file_without_login() {
    let mut server = Server::new();
    let _ability_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetAbility".to_string()),
            Matcher::UrlEncoded("token".to_string(), "cached-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetDevInfo":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _dev_info_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetDevInfo".to_string()),
            Matcher::UrlEncoded("token".to_string(), "cached-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"name":"cam"}}}]"#)
        .expect(1)
        .create();

    let token_cache_path = unique_temp_file_path("cli-token-cache", "session.token");
    cleanup_output_file(&token_cache_path);
    fs::create_dir_all(
        token_cache_path
            .parent()
            .expect("token cache path should include parent"),
    )
    .expect("token cache parent should be created");
    fs::write(&token_cache_path, "cached-token").expect("token cache should be created");

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-dev-info")
        .env("REOCLI_ENDPOINT", server.url())
        .env(
            "REOCLI_TOKEN_CACHE_PATH",
            token_cache_path.to_string_lossy().to_string(),
        )
        .env_remove("REOCLI_TOKEN")
        .env_remove("REOCLI_PASSWORD")
        .output()
        .expect("failed to run reocli get-dev-info with token cache");

    assert!(output.status.success());
    cleanup_output_file(&token_cache_path);
}

#[test]
fn reocli_refreshes_expired_token_cache_when_password_env_is_set() {
    let mut server = Server::new();
    let _ability_expired_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetAbility".to_string()),
            Matcher::UrlEncoded("token".to_string(), "expired-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":1,"error":{"detail":"please login first","rspCode":-6}}]"#,
        )
        .expect(1)
        .create();
    let _login_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Login".to_string()))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r#""userName":"admin""#.to_string()),
            Matcher::Regex(r#""password":"secret""#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Login","code":0,"value":{"Token":{"name":"fresh-token"}}}]"#)
        .expect(1)
        .create();
    let _ability_fresh_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetAbility".to_string()),
            Matcher::UrlEncoded("token".to_string(), "fresh-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetDevInfo":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _dev_info_expired_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetDevInfo".to_string()),
            Matcher::UrlEncoded("token".to_string(), "expired-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetDevInfo","code":1,"error":{"detail":"please login first","rspCode":-6}}]"#,
        )
        .expect(1)
        .create();
    let _dev_info_fresh_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetDevInfo".to_string()),
            Matcher::UrlEncoded("token".to_string(), "fresh-token".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"name":"cam"}}}]"#)
        .expect(1)
        .create();

    let token_cache_path = unique_temp_file_path("cli-token-refresh", "session.token");
    cleanup_output_file(&token_cache_path);
    fs::create_dir_all(
        token_cache_path
            .parent()
            .expect("token cache path should include parent"),
    )
    .expect("token cache parent should be created");
    fs::write(&token_cache_path, "expired-token").expect("token cache should be created");

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-dev-info")
        .env("REOCLI_ENDPOINT", server.url())
        .env(
            "REOCLI_TOKEN_CACHE_PATH",
            token_cache_path.to_string_lossy().to_string(),
        )
        .env("REOCLI_PASSWORD", "secret")
        .env_remove("REOCLI_TOKEN")
        .env_remove("REOCLI_USER")
        .output()
        .expect("failed to run reocli get-dev-info with token cache refresh");

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&token_cache_path).expect("token cache should be readable"),
        "fresh-token"
    );
    cleanup_output_file(&token_cache_path);
}

#[test]
fn reocli_get_ptz_status_works() {
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
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":1,"Ppos":1200,"Tpos":-80}}}]"#,
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

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-ptz-status")
        .arg("1")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli get-ptz-status");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("channel=1"));
    assert!(stdout.contains("pan=1200"));
    assert!(stdout.contains("tilt=-80"));
    assert!(!stdout.contains("pan_deg="));
    assert!(!stdout.contains("tilt_deg="));
}

#[test]
fn reocli_snap_with_out_path_works() {
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
    let _snap_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":0,"value":{"base64":"aGVsbG8="}}]"#)
        .expect(1)
        .create();

    let out_path = unique_temp_file_path("cli-snap", "channel-4.jpg");
    cleanup_output_file(&out_path);
    let out_string = out_path.to_string_lossy().into_owned();
    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("snap")
        .arg("4")
        .arg("--out")
        .arg(&out_string)
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli snap");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("channel=4"));
    assert!(stdout.contains("bytes_written=5"));
    assert_eq!(
        fs::read(&out_path).expect("snapshot file should be saved"),
        b"hello"
    );
    cleanup_output_file(&out_path);
}

#[test]
fn reocli_ptz_move_works() {
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
            Matcher::Regex(r#""channel":1"#.to_string()),
            Matcher::Regex(r#""op":"Left""#.to_string()),
            Matcher::Regex(r#""speed":5"#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("move")
        .arg("left")
        .arg("--speed")
        .arg("5")
        .arg("--channel")
        .arg("1")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli ptz move");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=move"));
}

#[test]
fn reocli_ptz_preset_list_works() {
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
            r#"[{"cmd":"GetAbility","code":0,"value":{"Ability":{"GetPtzPreset":{"permit":1}}}}]"#,
        )
        .expect(1)
        .create();
    let _preset_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzPreset".to_string(),
        ))
        .match_body(Matcher::Regex(r#""channel":1"#.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzPreset","code":0,"value":{"PtzPreset":[{"channel":1,"enable":1,"id":7,"name":"Home"}]}}]"#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("preset")
        .arg("list")
        .arg("--channel")
        .arg("1")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli ptz preset list");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("presets=[7:Home]"));
}

#[test]
fn reocli_ptz_calibrate_auto_works() {
    let mut server = Server::new();
    setup_ptz_absolute_mocks(&mut server);
    let calibration_dir = unique_temp_dir("cli-ptz-calibrate");
    cleanup_output_dir(&calibration_dir);

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("calibrate")
        .arg("auto")
        .arg("--channel")
        .arg("0")
        .env("REOCLI_ENDPOINT", server.url())
        .env("REOCLI_CALIBRATION_DIR", &calibration_dir)
        .output()
        .expect("failed to run reocli ptz calibrate auto");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=calibrate_auto"));
    assert!(stdout.contains("pan_count="));
    assert!(stdout.contains("tilt_count="));
    assert!(stdout.contains("pan_error_p95_count="));
    assert!(stdout.contains("tilt_error_p95_count="));
    assert!(!stdout.contains("pan_error_p95_deg="));
    assert!(!stdout.contains("tilt_error_p95_deg="));
    cleanup_output_dir(&calibration_dir);
}

#[test]
fn reocli_ptz_set_absolute_works() {
    let mut server = Server::new();
    setup_ptz_absolute_mocks(&mut server);

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("set-absolute")
        .arg("1500")
        .arg("-180")
        .arg("--tol-count")
        .arg("5")
        .arg("--timeout-ms")
        .arg("4000")
        .arg("--channel")
        .arg("0")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli ptz set-absolute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=set_absolute"));
    assert!(stdout.contains("pan_count=1500"));
    assert!(stdout.contains("tilt_count=-180"));
}

#[test]
fn reocli_ptz_get_absolute_works() {
    let mut server = Server::new();
    setup_ptz_absolute_mocks(&mut server);

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("get-absolute")
        .arg("--channel")
        .arg("0")
        .env("REOCLI_ENDPOINT", server.url())
        .output()
        .expect("failed to run reocli ptz get-absolute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=get_absolute"));
    assert!(stdout.contains("pan_count=1200"));
    assert!(stdout.contains("tilt_count=-80"));
}

#[test]
fn reocli_ptz_onvif_status_works() {
    let mut server = Server::new();
    setup_onvif_service_discovery_mocks(&mut server, "Profile000", Some("Ptz000"));
    let _status_mock = server
        .mock("POST", "/onvif/ptz_service")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("GetStatus".to_string()),
            Matcher::Regex("Profile000".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">
              <s:Body>
                <tptz:GetStatusResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
                  <tptz:PTZStatus>
                    <tt:Position xmlns:tt="http://www.onvif.org/ver10/schema">
                      <tt:PanTilt x="0.250000" y="-0.125000"/>
                    </tt:Position>
                    <tt:MoveStatus xmlns:tt="http://www.onvif.org/ver10/schema">
                      <tt:PanTilt>MOVING</tt:PanTilt>
                    </tt:MoveStatus>
                    <tt:UtcTime xmlns:tt="http://www.onvif.org/ver10/schema">2026-02-28T00:00:00Z</tt:UtcTime>
                  </tptz:PTZStatus>
                </tptz:GetStatusResponse>
              </s:Body>
            </s:Envelope>
            "#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("onvif")
        .arg("status")
        .arg("--channel")
        .arg("0")
        .env("REOCLI_PTZ_BACKEND", "onvif")
        .env("REOCLI_ENDPOINT", server.url())
        .env(
            "REOCLI_ONVIF_DEVICE_SERVICE_URL",
            format!("{}/onvif/device_service", server.url()),
        )
        .env("REOCLI_USER", "admin")
        .env("REOCLI_PASSWORD", "secret")
        .output()
        .expect("failed to run reocli ptz onvif status");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=onvif_status"));
    assert!(stdout.contains("pan=0.250000"));
    assert!(stdout.contains("tilt=-0.125000"));
    assert!(stdout.contains("pan_tilt_move_status=moving"));
}

#[test]
fn reocli_ptz_onvif_options_works() {
    let mut server = Server::new();
    setup_onvif_service_discovery_mocks(&mut server, "Profile000", Some("Ptz000"));
    let _options_mock = server
        .mock("POST", "/onvif/ptz_service")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("GetConfigurationOptions".to_string()),
            Matcher::Regex("Ptz000".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">
              <s:Body>
                <tptz:GetConfigurationOptionsResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
                  <tptz:PTZConfigurationOptions>
                    <tt:Spaces xmlns:tt="http://www.onvif.org/ver10/schema">
                      <tt:ContinuousPanTiltVelocitySpace/>
                      <tt:RelativePanTiltTranslationSpace/>
                      <tt:PanTiltSpeedSpace/>
                    </tt:Spaces>
                    <tt:PTZTimeout xmlns:tt="http://www.onvif.org/ver10/schema">
                      <tt:Min>PT0S</tt:Min>
                      <tt:Max>PT10S</tt:Max>
                    </tt:PTZTimeout>
                  </tptz:PTZConfigurationOptions>
                </tptz:GetConfigurationOptionsResponse>
              </s:Body>
            </s:Envelope>
            "#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("onvif")
        .arg("options")
        .arg("--channel")
        .arg("0")
        .env("REOCLI_PTZ_BACKEND", "onvif")
        .env("REOCLI_ENDPOINT", server.url())
        .env(
            "REOCLI_ONVIF_DEVICE_SERVICE_URL",
            format!("{}/onvif/device_service", server.url()),
        )
        .env("REOCLI_USER", "admin")
        .env("REOCLI_PASSWORD", "secret")
        .output()
        .expect("failed to run reocli ptz onvif options");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=onvif_options"));
    assert!(stdout.contains("supports_relative_pan_tilt_translation=true"));
    assert!(stdout.contains("timeout_min=PT0S"));
    assert!(stdout.contains("timeout_max=PT10S"));
}

#[test]
fn reocli_ptz_onvif_relative_move_works() {
    let mut server = Server::new();
    setup_onvif_service_discovery_mocks(&mut server, "Profile000", Some("Ptz000"));
    let _options_mock = server
        .mock("POST", "/onvif/ptz_service")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("GetConfigurationOptions".to_string()),
            Matcher::Regex("Ptz000".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">
              <s:Body>
                <tptz:GetConfigurationOptionsResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
                  <tptz:PTZConfigurationOptions>
                    <tt:Spaces xmlns:tt="http://www.onvif.org/ver10/schema">
                      <tt:RelativePanTiltTranslationSpace/>
                    </tt:Spaces>
                  </tptz:PTZConfigurationOptions>
                </tptz:GetConfigurationOptionsResponse>
              </s:Body>
            </s:Envelope>
            "#,
        )
        .expect(1)
        .create();
    let _relative_move_mock = server
        .mock("POST", "/onvif/ptz_service")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("RelativeMove".to_string()),
            Matcher::Regex("Profile000".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope">
              <s:Body>
                <tptz:RelativeMoveResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl"/>
              </s:Body>
            </s:Envelope>
            "#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("ptz")
        .arg("onvif")
        .arg("relative-move")
        .arg("40")
        .arg("-12")
        .arg("--channel")
        .arg("0")
        .env("REOCLI_PTZ_BACKEND", "onvif")
        .env("REOCLI_ENDPOINT", server.url())
        .env(
            "REOCLI_ONVIF_DEVICE_SERVICE_URL",
            format!("{}/onvif/device_service", server.url()),
        )
        .env("REOCLI_USER", "admin")
        .env("REOCLI_PASSWORD", "secret")
        .output()
        .expect("failed to run reocli ptz onvif relative-move");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("operation=onvif_relative_move"));
    assert!(stdout.contains("pan_delta_count=40"));
    assert!(stdout.contains("tilt_delta_count=-12"));
    assert!(stdout.contains("applied=true"));
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

fn setup_onvif_service_discovery_mocks(
    server: &mut Server,
    profile_token: &str,
    ptz_configuration_token: Option<&str>,
) {
    let media_service_url = format!("{}/onvif/media_service", server.url());
    let ptz_service_url = format!("{}/onvif/ptz_service", server.url());
    let mut profiles_body = format!(r#"<trt:Profiles token="{profile_token}">"#);
    if let Some(token) = ptz_configuration_token {
        profiles_body.push_str(&format!(r#"<tt:PTZConfiguration token="{token}"/>"#));
    }
    profiles_body.push_str("</trt:Profiles>");

    let _capabilities_mock = server
        .mock("POST", "/onvif/device_service")
        .match_body(Matcher::Regex("GetCapabilities".to_string()))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(format!(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tds="http://www.onvif.org/ver10/device/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema">
              <s:Body>
                <tds:GetCapabilitiesResponse>
                  <tds:Capabilities>
                    <tt:Media><tt:XAddr>{media_service_url}</tt:XAddr></tt:Media>
                    <tt:PTZ><tt:XAddr>{ptz_service_url}</tt:XAddr></tt:PTZ>
                  </tds:Capabilities>
                </tds:GetCapabilitiesResponse>
              </s:Body>
            </s:Envelope>
            "#
        ))
        .expect(1)
        .create();
    let _profiles_mock = server
        .mock("POST", "/onvif/media_service")
        .match_body(Matcher::Regex("GetProfiles".to_string()))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(format!(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:trt="http://www.onvif.org/ver10/media/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema">
              <s:Body>
                <trt:GetProfilesResponse>
                  {profiles_body}
                </trt:GetProfilesResponse>
              </s:Body>
            </s:Envelope>
            "#
        ))
        .expect(1)
        .create();
}
