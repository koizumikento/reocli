use mockito::{Matcher, Server};
use reocli::app::usecases;
use reocli::core::command::{CgiCommand, CommandRequest};
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};

#[test]
fn execute_returns_response_on_success() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"GetDevInfo","code":0,"value":{"DevInfo":{"model":"RLC"}}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let response = client
        .execute(CommandRequest::new(CgiCommand::GetDevInfo))
        .expect("expected successful response");

    assert_eq!(response.command, CgiCommand::GetDevInfo);
    assert!(response.raw_json.contains("\"code\":0"));
}

#[test]
fn execute_returns_authentication_error_on_http_401() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(401)
        .with_body("unauthorized")
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = client
        .execute(CommandRequest::new(CgiCommand::GetDevInfo))
        .expect_err("expected authentication error");

    assert_eq!(error.kind, ErrorKind::Authentication);
    assert!(error.message.contains("status=401"));
}

#[test]
fn execute_returns_network_error_on_non_success_status() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(503)
        .with_body("service unavailable")
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = client
        .execute(CommandRequest::new(CgiCommand::GetDevInfo))
        .expect_err("expected network error");

    assert_eq!(error.kind, ErrorKind::Network);
    assert!(error.message.contains("status=503"));
}

#[test]
fn get_time_returns_unexpected_response_on_invalid_json() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetTime".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("not-json")
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = usecases::get_time::execute(&client).expect_err("expected parse failure");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(
        error
            .message
            .contains("failed to parse GetTime response JSON")
    );
}
