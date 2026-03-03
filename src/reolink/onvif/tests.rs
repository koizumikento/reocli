use super::*;

#[test]
fn extract_media_profiles_parses_multiple_entries() {
    let xml = r#"
    <trt:GetProfilesResponse xmlns:trt="http://www.onvif.org/ver10/media/wsdl">
      <trt:Profiles token="Profile000"></trt:Profiles>
      <trt:Profiles token='Profile001'></trt:Profiles>
      <trt:Profiles></trt:Profiles>
    </trt:GetProfilesResponse>
    "#;
    let tokens = extract_media_profiles(xml)
        .into_iter()
        .map(|profile| profile.token)
        .collect::<Vec<_>>();
    assert_eq!(
        tokens,
        vec!["Profile000".to_string(), "Profile001".to_string()]
    );
}

#[test]
fn extract_media_profiles_parses_ptz_configuration_tokens() {
    let xml = r#"
    <trt:GetProfilesResponse xmlns:trt="http://www.onvif.org/ver10/media/wsdl">
      <trt:Profiles token="Profile000">
        <tt:PTZConfiguration token="Ptz000"/>
      </trt:Profiles>
      <trt:Profiles token="Profile001">
        <tt:PTZConfiguration token='Ptz001'/>
      </trt:Profiles>
    </trt:GetProfilesResponse>
    "#;

    let profiles = extract_media_profiles(xml);
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].token, "Profile000");
    assert_eq!(
        profiles[0].ptz_configuration_token.as_deref(),
        Some("Ptz000")
    );
    assert_eq!(profiles[1].token, "Profile001");
    assert_eq!(
        profiles[1].ptz_configuration_token.as_deref(),
        Some("Ptz001")
    );
}

#[test]
fn parse_get_status_response_parses_position_move_status_and_utc_time() {
    let xml = r#"
    <tptz:GetStatusResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
      <tptz:PTZStatus>
        <tt:Position>
          <tt:PanTilt x="0.250" y="-0.125"/>
          <tt:Zoom x="0.500"/>
        </tt:Position>
        <tt:MoveStatus>
          <tt:PanTilt>MOVING</tt:PanTilt>
          <tt:Zoom>IDLE</tt:Zoom>
        </tt:MoveStatus>
        <tt:UtcTime>2026-02-20T10:11:12Z</tt:UtcTime>
      </tptz:PTZStatus>
    </tptz:GetStatusResponse>
    "#;

    let status = parse_get_status_response(xml);
    assert_eq!(status.pan, Some(0.25));
    assert_eq!(status.tilt, Some(-0.125));
    assert_eq!(status.zoom, Some(0.5));
    assert_eq!(status.pan_tilt_move_status, Some(OnvifMoveStatus::Moving));
    assert_eq!(status.zoom_move_status, Some(OnvifMoveStatus::Idle));
    assert_eq!(status.utc_time.as_deref(), Some("2026-02-20T10:11:12Z"));
}

#[test]
fn parse_get_configuration_options_response_parses_timeout_and_flags() {
    let xml = r#"
    <tptz:GetConfigurationOptionsResponse xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl">
      <tptz:PTZConfigurationOptions>
        <tt:Spaces>
          <tt:RelativePanTiltTranslationSpace/>
          <tt:ContinuousPanTiltVelocitySpace/>
          <tt:PanTiltSpeedSpace/>
        </tt:Spaces>
        <tt:PTZTimeout>
          <tt:Min>PT0S</tt:Min>
          <tt:Max>PT5S</tt:Max>
        </tt:PTZTimeout>
      </tptz:PTZConfigurationOptions>
    </tptz:GetConfigurationOptionsResponse>
    "#;

    let options = parse_get_configuration_options_response(xml);
    assert!(options.supports_relative_pan_tilt_translation);
    assert!(options.supports_continuous_pan_tilt_velocity);
    assert!(options.supports_relative_pan_tilt_speed);
    assert!(options.has_timeout_range);
    assert_eq!(options.timeout_min.as_deref(), Some("PT0S"));
    assert_eq!(options.timeout_max.as_deref(), Some("PT5S"));
}

#[test]
fn normalize_relative_translation_scales_to_speed_space() {
    let (pan, tilt) = normalize_relative_translation(4.0, -2.0, 32).expect("normalized");
    assert!((pan - 0.5).abs() < 1e-9);
    assert!((tilt + 0.25).abs() < 1e-9);
}

#[test]
fn build_relative_move_body_contains_translation_and_speed_vectors() {
    let body = build_relative_move_body("Profile<1>", 0.25, -0.5);
    assert!(body.contains("<tptz:RelativeMove"));
    assert!(body.contains("<tptz:ProfileToken>Profile&lt;1&gt;</tptz:ProfileToken>"));
    assert!(body.contains("<tptz:Translation>"));
    assert!(body.contains("<tptz:Speed>"));
    assert!(body.contains("x=\"0.250000\" y=\"-0.500000\""));
}

#[test]
fn build_get_configuration_options_body_escapes_configuration_token() {
    let body = build_get_configuration_options_body("Token&01");
    assert!(body.contains("<tptz:ConfigurationToken>Token&amp;01</tptz:ConfigurationToken>"));
}

#[test]
fn build_get_status_body_escapes_profile_token() {
    let body = build_get_status_body("Profile\"A\"");
    assert!(body.contains("<tptz:ProfileToken>Profile&quot;A&quot;</tptz:ProfileToken>"));
}

#[test]
fn velocity_from_direction_rejects_diagonal() {
    let error = velocity_from_direction(PtzDirection::LeftUp, 8).expect_err("must fail");
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(error.message.contains("diagonal"));
}
