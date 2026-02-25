use mockito::{Matcher, Server};
use reocli::core::error::ErrorKind;
use reocli::reolink::auth;
use reocli::reolink::client::{Auth, Client};

#[test]
fn get_user_auth_returns_token_on_success() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetUserAuth".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetUserAuth","code":0,"value":{"Token":{"name":"token-from-name"}}}]"#,
        )
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let token = auth::get_user_auth(&client, "admin", "secret").expect("expected success");

    assert_eq!(token, "token-from-name");
}

#[test]
fn get_user_auth_returns_error_when_code_is_non_zero() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetUserAuth".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetUserAuth","code":1,"value":{"Token":{"name":"ignored"}}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = auth::get_user_auth(&client, "admin", "secret").expect_err("expected error");

    assert_eq!(error.kind, ErrorKind::Authentication);
    assert!(error.message.contains("non-zero code"));
}

#[test]
fn get_user_auth_returns_error_when_token_is_missing() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetUserAuth".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetUserAuth","code":0,"value":{"Token":{"leaseTime":3600}}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = auth::get_user_auth(&client, "admin", "secret").expect_err("expected error");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("token was not found"));
}
