use mockito::{Matcher, Server};
use reocli::core::error::ErrorKind;
use reocli::core::model::PtzDirection;
use reocli::reolink::onvif::{self, OnvifConfig};

#[test]
fn continuous_move_resolves_services_and_profile_token() {
    let mut server = Server::new();
    let base_url = server.url();
    let device_service_url = format!("{base_url}/onvif/device_service");
    let media_service_url = format!("{base_url}/onvif/media_service");
    let ptz_service_url = format!("{base_url}/onvif/ptz_service");

    let _capabilities_mock = server
        .mock("POST", "/onvif/device_service")
        .match_body(Matcher::Regex("GetCapabilities".to_string()))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(format!(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tds="http://www.onvif.org/ver10/device/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema">
              <s:Body>
                <tds:GetCapabilitiesResponse>
                  <tds:Capabilities>
                    <tt:Media><tt:XAddr>{media_service_url}</tt:XAddr></tt:Media>
                    <tt:PTZ><tt:XAddr>{ptz_service_url}</tt:XAddr></tt:PTZ>
                  </tds:Capabilities>
                </tds:GetCapabilitiesResponse>
              </s:Body>
            </s:Envelope>
            "#
        ))
        .expect(1)
        .create();

    let _profiles_mock = server
        .mock("POST", "/onvif/media_service")
        .match_body(Matcher::Regex("GetProfiles".to_string()))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"
            <s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:trt="http://www.onvif.org/ver10/media/wsdl">
              <s:Body>
                <trt:GetProfilesResponse>
                  <trt:Profiles token="Profile000"></trt:Profiles>
                  <trt:Profiles token="Profile001"></trt:Profiles>
                </trt:GetProfilesResponse>
              </s:Body>
            </s:Envelope>
            "#,
        )
        .expect(1)
        .create();

    let _continuous_mock = server
        .mock("POST", "/onvif/ptz_service")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("ContinuousMove".to_string()),
            Matcher::Regex("Profile001".to_string()),
            Matcher::Regex(r#"PanTilt x="-0\.125000" y="0\.000000""#.to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><tptz:ContinuousMoveResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl"/></s:Body></s:Envelope>"#,
        )
        .expect(1)
        .create();

    let _stop_mock = server
        .mock("POST", "/onvif/ptz_service")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("<tptz:Stop".to_string()),
            Matcher::Regex("Profile001".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/soap+xml; charset=utf-8")
        .with_body(
            r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><tptz:StopResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl"/></s:Body></s:Envelope>"#,
        )
        .expect(1)
        .create();

    let config = OnvifConfig::with_defaults(device_service_url, "admin", "secret", None);
    onvif::continuous_move(&config, 1, PtzDirection::Left, 8, Some(0))
        .expect("ContinuousMove should succeed");
}

#[test]
fn continuous_move_rejects_diagonal_direction() {
    let config = OnvifConfig::with_defaults(
        "http://127.0.0.1:8000/onvif/device_service",
        "admin",
        "secret",
        Some("Profile000".to_string()),
    );
    let error = onvif::continuous_move(&config, 0, PtzDirection::LeftUp, 8, Some(80))
        .expect_err("diagonal direction must be rejected");

    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(error.message.contains("diagonal"));
}
