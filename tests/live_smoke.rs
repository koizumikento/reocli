use std::process::Command;

#[test]
fn live_get_dev_info_when_enabled() {
    if std::env::var("REOCLI_LIVE_TEST").ok().as_deref() != Some("1") {
        return;
    }

    let output = Command::new(env!("CARGO_BIN_EXE_reocli"))
        .arg("get-dev-info")
        .output()
        .expect("failed to run live get-dev-info");

    assert!(
        output.status.success(),
        "live get-dev-info failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
