#[test]
fn colors_are_optional_and_wire_compatible() -> anyhow::Result<()> {
    use spatial_home::mobile_depth::Sample;
    let old = r#"{"session":"s","timestamp_ns":1,"camera_to_session":{"from_frame_id":"a","to_frame_id":"b","translation":[0.0,0.0,0.0],"rotation_wxyz":[1.0,0.0,0.0,0.0]},"alignment":{"timestamp_ns":1,"portal_id":"P","portal_to_session":{"from_frame_id":"a","to_frame_id":"b","translation":[0.0,0.0,0.0],"rotation_wxyz":[1.0,0.0,0.0,0.0]},"session_to_home":{"from_frame_id":"a","to_frame_id":"b","translation":[0.0,0.0,0.0],"rotation_wxyz":[1.0,0.0,0.0,0.0]}},"points":[[0.0,0.0,-1.0]]}"#;
    let mut sample: Sample = serde_json::from_str(old)?;
    assert!(sample.colors.is_none());
    assert!(!serde_json::to_string(&sample)?.contains("colors"));
    sample.colors = Some(vec![[10, 20, 30]]);
    let colored: Sample = serde_json::from_str(&serde_json::to_string(&sample)?)?;
    assert_eq!(colored.colors, Some(vec![[10, 20, 30]]));
    Ok(())
}
