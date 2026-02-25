use mockito::{Matcher, Server};
use reocli::core::command::{CgiCommand, CommandRequest};
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};

#[test]
fn token_auth_falls_back_to_user_password_when_login_is_required() {
    let mut server = Server::new();

    let _token_request = server
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

    let _fallback_request = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cmd".to_string(), "GetDevInfo".to_string()),
            Matcher::UrlEncoded("user".to_string(), "admin".to_string()),
            Matcher::UrlEncoded("password".to_string(), "secret".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"model":"RLC"}}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Token("expired-token".to_string()))
        .with_fallback_auth(Auth::UserPassword {
            user: "admin".to_string(),
            password: "secret".to_string(),
        });

    let response = client
        .execute(CommandRequest::new(CgiCommand::GetDevInfo))
        .expect("token authentication should fall back to user/password");

    assert!(response.raw_json.contains("\"code\":0"));
}

#[test]
fn token_auth_without_fallback_returns_authentication_error() {
    let mut server = Server::new();

    let _token_request = server
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

    let client = Client::new(server.url(), Auth::Token("expired-token".to_string()));
    let error = client
        .execute(CommandRequest::new(CgiCommand::GetDevInfo))
        .expect_err("token authentication should fail without fallback");

    assert_eq!(error.kind, ErrorKind::Authentication);
}
