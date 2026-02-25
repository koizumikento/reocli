use std::process::Command;

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
