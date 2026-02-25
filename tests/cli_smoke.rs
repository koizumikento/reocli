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
