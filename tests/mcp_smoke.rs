use std::process::Command;

use mockito::{Matcher, Server};

#[test]
fn reocli_mcp_lists_tools() {
    let output = Command::new(env!("CARGO_BIN_EXE_reocli-mcp"))
        .output()
        .expect("failed to run reocli-mcp");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mcp.list_tools"));
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
    assert!(stdout.contains("\"channel\":1"));
}
