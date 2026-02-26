use mockito::{Matcher, Server};
use reocli::app::usecases::snap;
use reocli::core::error::ErrorKind;
use reocli::reolink::client::{Auth, Client};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn snap_saves_image_to_explicit_output_path() {
    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"Snap","code":0,"value":{"path":"tmp/snap/ch1.jpg","base64":"aGVsbG8="}}]"#,
        )
        .expect(1)
        .create();

    let output_path = unique_temp_file_path("snap-explicit", "channel-1.jpg");
    cleanup_output_file(&output_path);
    let output_string = output_path.to_string_lossy().into_owned();

    let client = Client::new(server.url(), Auth::Anonymous);
    let snapshot =
        snap::execute_with_out_path(&client, 1, Some(&output_string)).expect("snap should succeed");

    assert_eq!(snapshot.channel, 1);
    assert_eq!(snapshot.image_path, output_string);
    assert_eq!(snapshot.bytes_written, 5);
    assert_eq!(
        fs::read(&output_path).expect("file should be written"),
        b"hello"
    );
    cleanup_output_file(&output_path);
}

#[test]
fn snap_uses_default_output_path_when_none_is_provided() {
    let channel = 22;
    let expected_path = PathBuf::from(format!("snapshots/channel-{channel}.jpg"));
    remove_file_if_exists(&expected_path);

    let mut server = Server::new();
    let _mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded("cmd".to_string(), "Snap".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"Snap","code":0,"value":{"bytes":"abc123"}}]"#)
        .expect(1)
        .create();

    let client = Client::new(server.url(), Auth::Anonymous);
    let snapshot =
        snap::execute_with_out_path(&client, channel, None).expect("snap should succeed");

    assert_eq!(snapshot.channel, channel);
    assert_eq!(
        snapshot.image_path,
        expected_path.to_string_lossy().into_owned()
    );
    assert_eq!(snapshot.bytes_written, 6);
    assert_eq!(
        fs::read(&expected_path).expect("file should be written"),
        b"abc123"
    );
    remove_file_if_exists(&expected_path);
}

#[test]
fn snap_returns_error_when_response_code_is_non_zero() {
    let output_path = unique_temp_file_path("snap-error", "channel-3.jpg");
    cleanup_output_file(&output_path);
    let output_string = output_path.to_string_lossy().into_owned();

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
    let error = snap::execute_with_out_path(&client, 3, Some(&output_string))
        .expect_err("snap should fail for non-zero code");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("code=1"));
    assert!(!output_path.exists());
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
    remove_file_if_exists(path);
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir_all(parent);
    }
}

fn remove_file_if_exists(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}
