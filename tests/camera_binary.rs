use spatial_home::camera_frame::*;
use auki_components::VideoFrame;
#[test]
fn binary_preserves_image_without_json_and_rejects_malformed_frames()->anyhow::Result<()> {
    for (encoding,bytes) in [("rgb8",vec![255;12]),("luma8",vec![0,10,34,255])] {
        let frame=VideoFrame {width:2,height:2,encoding:encoding.into(),bytes:bytes.clone().into()};
        let packet=pack(&frame)?;
        assert_eq!(packet.len(),16+bytes.len());assert_eq!(&packet[16..],bytes);
        assert_eq!(unpack(&packet)?,frame);
        assert!(unpack(&packet[..16]).is_err());
        let mut bad=packet.clone();bad[12]=99;assert!(unpack(&bad).is_err());
        let mut bad=packet.clone();bad[13]=1;assert!(unpack(&bad).is_err());
        let mut bad=packet;bad[4..8].copy_from_slice(&4097u32.to_le_bytes());assert!(unpack(&bad).is_err());
    }
    assert!(decoder("unknown").is_err());
    let mut jpeg=vec![];image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image::GrayImage::new(64,64))?;
    let mut frame=VideoFrame {width:64,height:64,encoding:"jpeg".into(),bytes:jpeg.clone().into()};
    assert_eq!(unpack(&pack(&frame)?)?,frame);frame.width=65;assert!(pack(&frame).is_err());
    let meta=serde_json::json!({"capture_timestamp_ns":"9007199254740993","sequence":"7","encoding":"jpeg","width":64,"height":64,"clock":{"peer_id":"peer","id":"clock","hash":"hash"}});
    let browser=browser_packet(&meta,&jpeg)?;
    let n=u32::from_le_bytes(browser[..4].try_into().unwrap()) as usize;
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&browser[4..4+n])?,meta);
    assert_eq!(&browser[4+n..],jpeg);
    Ok(())
}

#[test]
fn dense_home_map_does_not_overflow_annotation_only_camera_response()->anyhow::Result<()> {
    use serde_json::json;
    use spatial_home::ar_preview::{project_annotations,project_view};
    let voxels:Vec<_>=(0..1024).map(|i|json!({
        "center":[((i%32) as f64-15.5)*0.03,((i/32) as f64-15.5)*0.03,2.],
        "evidence":1.,"linear_rgb":[0.1,0.6,0.8]
    })).collect();
    let mut view=json!({"frame":"room","voxel_size":0.03,"voxels":voxels,
        "notes":[{"id":"note","text":"Measured note","position":[0.,0.,2.]}],
        "paths":{"schema":spatial_home::scene_paths::SCHEMA,"frame":"room","items":[{
            "id":"00000000-0000-4000-8000-000000000001","title":"Drawing","color":"#ffca55",
            "width_m":0.035,"segments":[[[-0.1,0.,2.],[0.1,0.,2.]]]
        }]}});
    let calibration=json!({"width":640,"height":480,"K":[300.,0.,320.,0.,300.,240.,0.,0.,1.],"D":[],"distortion_model":"pinhole"});
    let pose=spatial_home::local_mapper::identity();
    let old=project_view(&view,&pose,&calibration)?;
    assert_eq!(old["voxel_count"],1024);
    // Reproduce the regression with real projected triangles, not padding.
    assert!(browser_packet(&json!({"ar":old}),&[7]).unwrap_err().to_string().contains("overlay metadata"));
    let annotations=project_annotations(&view,&pose,&calibration)?;
    assert_eq!(annotations["vertices"],json!([]));
    assert_eq!(annotations["voxel_count"],0);
    assert_eq!(annotations["notes"],old["notes"]);
    assert_eq!(annotations["path_segments"],old["path_segments"]);
    assert_eq!(annotations["notes"][0]["pixel"],json!([320.,240.]));
    assert_eq!(annotations["path_segments"].as_array().unwrap().len(),1);
    let metadata=json!({"ar":annotations,"sequence":"73","capture_timestamp_ns":"9007199254740993",
        "clock":{"peer_id":"any-peer","id":"capture","hash":"same-clock"},"camera_token":"capture-bound-pick"});
    let packet=browser_packet(&metadata,&[7,42,255])?;
    let n=u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize;
    assert!(n<4096);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&packet[4..4+n])?,metadata);
    assert_eq!(&packet[4+n..],&[7,42,255]);
    assert_eq!(view["voxels"].as_array().unwrap().len(),1024);
    // Camera metadata is independent of voxel map growth and never mutates it.
    view["voxels"]=json!([]);
    assert_eq!(project_annotations(&view,&pose,&calibration)?,annotations);
    Ok(())
}

#[test]
fn camera_limits_distinguish_overlay_metadata_from_image_bytes() {
    let metadata=serde_json::json!({"oversized":"x".repeat(1024*1024)});
    assert!(browser_packet(&metadata,&[1]).unwrap_err().to_string().contains("overlay metadata"));
    assert!(browser_packet(&serde_json::json!({}),&vec![0;MAX_BYTES+1]).unwrap_err().to_string().contains("image exceeds"));
}
