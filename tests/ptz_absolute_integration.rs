use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mockito::{Matcher, Server};
use reocli::app::usecases::ptz_calibrate_auto::StoredCalibration;
use reocli::app::usecases::ptz_set_absolute;
use reocli::core::error::ErrorKind;
use reocli::core::model::{AxisModelParams, CalibrationParams};
use reocli::reolink::client::{Auth, Client};

#[test]
fn set_absolute_success_path_sends_stop() {
    let unique = unique_suffix();
    let serial = format!("SERIAL-{unique}");
    let model = format!("Model-{unique}");
    let firmware = format!("v1.{unique}");
    let calibration_path = write_calibration_file(&serial, &model, &firmware, 0)
        .expect("failed to write calibration file");

    let mut server = Server::new();
    let _dev_info_mock = mock_dev_info(&mut server, &serial, &model, &firmware, 1);
    let _cur_pos_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCurPos".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":0,"Ppos":0,"Tpos":0}}}]"#,
        )
        .expect(1)
        .create();
    let _stop_mock = stop_mock(&mut server, 0, 1);

    let client = Client::new(server.url(), Auth::Anonymous);
    let result =
        ptz_set_absolute::execute(&client, 0, 0.0, 0.0, 0.5, 200).expect("set_absolute failed");

    assert_eq!(result.channel, 0);
    assert!(result.pan_deg.abs() <= 0.5);
    assert!(result.tilt_deg.abs() <= 0.5);

    cleanup_file(&calibration_path);
}

#[test]
fn set_absolute_timeout_path_sends_stop_and_returns_error() {
    let unique = unique_suffix();
    let serial = format!("SERIAL-{unique}");
    let model = format!("Model-{unique}");
    let firmware = format!("v1.{unique}");
    let calibration_path = write_calibration_file(&serial, &model, &firmware, 0)
        .expect("failed to write calibration file");

    let mut server = Server::new();
    let _dev_info_mock = mock_dev_info(&mut server, &serial, &model, &firmware, 1);
    let _cur_pos_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetPtzCurPos".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"cmd":"GetPtzCurPos","code":0,"value":{"PtzCurPos":{"channel":0,"Ppos":0,"Tpos":0}}}]"#,
        )
        .expect_at_least(1)
        .expect_at_most(5)
        .create();
    let _move_mock = server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::Regex(
            r#""op":"(Left|Right|Up|Down|LeftUp|LeftDown|RightUp|RightDown)""#.to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect_at_least(1)
        .expect_at_most(8)
        .create();
    let _stop_mock = stop_mock(&mut server, 0, 1);

    let client = Client::new(server.url(), Auth::Anonymous);
    let error = ptz_set_absolute::execute(&client, 0, 90.0, 45.0, 0.5, 80)
        .expect_err("set_absolute should timeout");

    assert_eq!(error.kind, ErrorKind::UnexpectedResponse);
    assert!(error.message.contains("timeout"));

    cleanup_file(&calibration_path);
}

fn mock_dev_info(
    server: &mut Server,
    serial: &str,
    model: &str,
    firmware: &str,
    expected_calls: usize,
) -> mockito::Mock {
    server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "GetDevInfo".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{"cmd":"GetDevInfo","code":0,"value":{{"DevInfo":{{"serial":"{serial}","model":"{model}","firmware":"{firmware}"}}}}}}]"#
        ))
        .expect(expected_calls)
        .create()
}

fn stop_mock(server: &mut Server, channel: u8, expected_calls: usize) -> mockito::Mock {
    server
        .mock("POST", "/cgi-bin/api.cgi")
        .match_query(Matcher::UrlEncoded(
            "cmd".to_string(),
            "PtzCtrl".to_string(),
        ))
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(format!(r#""channel":{channel}"#)),
            Matcher::Regex(r#""op":"Stop""#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"cmd":"PtzCtrl","code":0}]"#)
        .expect(expected_calls)
        .create()
}

fn write_calibration_file(
    serial: &str,
    model: &str,
    firmware: &str,
    channel: u8,
) -> Result<PathBuf, std::io::Error> {
    let camera_key = calibration_camera_key(serial, model, firmware);
    let path = calibration_dir().join(format!("{camera_key}.json"));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let stored = StoredCalibration {
        schema_version: 1,
        source: "test".to_string(),
        camera_key,
        channel,
        calibration: CalibrationParams {
            serial_number: serial.to_string(),
            model: model.to_string(),
            firmware: firmware.to_string(),
            pan_offset: 0.0,
            pan_scale: 0.18,
            pan_deadband: 0.1,
            tilt_offset: 0.0,
            tilt_scale: 0.18,
            tilt_deadband: 0.1,
            pan_model: AxisModelParams {
                alpha: 0.9,
                beta: 0.4,
            },
            tilt_model: AxisModelParams {
                alpha: 0.9,
                beta: 0.4,
            },
            created_at: "0".to_string(),
        },
    };

    let content = serde_json::to_string_pretty(&stored)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(&path, content)?;
    Ok(path)
}

fn calibration_dir() -> PathBuf {
    if let Ok(path) = std::env::var("REOCLI_CALIBRATION_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join(".reocli/calibration");
        }
    }

    PathBuf::from(".reocli/calibration")
}

fn calibration_camera_key(serial: &str, model: &str, firmware: &str) -> String {
    format!(
        "{}__{}__{}",
        sanitize_component(serial),
        sanitize_component(model),
        sanitize_component(firmware)
    )
}

fn sanitize_component(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut previous_was_separator = false;

    for character in raw.trim().chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            normalized.push('_');
            previous_was_separator = true;
        }
    }

    let trimmed = normalized.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn unique_suffix() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{now}-{}", std::process::id())
}

fn cleanup_file(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}
