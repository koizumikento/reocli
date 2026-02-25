use mockito::{Matcher, Server};
use reocli::app::usecases::snap;
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};

#[test]
fn snap_returns_path_from_response_value() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":0,"value":{"path":"tmp/snap/ch1.jpg"}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let snapshot = snap::execute(&client, 1).expect("snap should succeed");

    assert_eq!(snapshot.channel, 1);
    assert_eq!(snapshot.image_path, "tmp/snap/ch1.jpg");
}

#[test]
fn snap_falls_back_when_path_is_missing() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":0,"value":{"foo":"bar"}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let snapshot = snap::execute(&client, 2).expect("snap should succeed");

    assert_eq!(snapshot.channel, 2);
    assert_eq!(snapshot.image_path, "snapshots/channel-2.jpg");
}

#[test]
fn snap_returns_error_when_response_code_is_non_zero() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":1,"value":{"path":"tmp/snap/ch3.jpg"}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = snap::execute(&client, 3).expect_err("snap should fail for non-zero code");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("code=1"));
}
