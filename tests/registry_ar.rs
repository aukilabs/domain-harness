#[allow(dead_code)]
mod support;
use anyhow::Result;
use auki_components::*;
use auki_odometry::{Pose, Tracking};
use auki_registry::FrameRegistryEntry as Frame;
use serde_json::json;
use spatial_home::{
    compact_pose::{Contract, Lifetime, Update},
    product_registry as registry, registry_ar,
};

fn transform(
    runtime: &ComponentRuntime,
    endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
    from: &Frame,
    to: &Frame,
    clock: &ClockReference,
    lifetime: Lifetime,
    name: &str,
) -> Result<(BufferProductCapture<Update>, ConfiguredObservable<Update>)> {
    let c = Contract {
        from_frame: from.into(),
        to_frame: to.into(),
        clock: clock.clone(),
        session_id: "s".into(),
        lifetime,
    };
    let component = runtime.component(ComponentSpec::new(name).observable(ObservableContract {
        name: "value".into(),
        datatype: Update::DATATYPE.into(),
        schema: Update::DATATYPE.into(),
        access: vec![ObservationAccess::FollowNew],
        exposure: Exposure::Cluster,
    }))?;
    let output = component.configured_observable(
        ConfiguredObservableSpec::new(
            "value",
            c.output_id(),
            clock.clone(),
            PayloadContract::Structured(StructuredPayloadContract {
                datatype: Update::DATATYPE.into(),
                schema: Update::DATATYPE.into(),
                modality: "pose".into(),
                observes: "declared relationship".into(),
                unit: None,
            }),
        )
        .in_registered_frame(to.clone()),
    )?;
    component.expose()?;
    let capture = runtime.capture_buffer_with_metadata(
        format!("{name}-history"),
        &output,
        BufferLimits {
            max_entries: Some(32),
            max_bytes: None,
            target_duration: None,
        },
        |_: &Update| 256,
        move |o| {
            Ok(Some(CatalogProductMetadata {
                schema: spatial_home::compact_pose::METADATA.into(),
                source_sequence: o.sequence,
                value: serde_json::to_value(&c).map_err(|e| e.to_string())?,
            }))
        },
    )?;
    endpoint.export_product(&capture.product())?;
    Ok((capture, output))
}

fn valid(x: f64) -> std::sync::Arc<Update> {
    Update {
        tracking: Tracking::Valid {
            pose: Pose {
                translation: [x, 0., 0.],
                rotation_xyzw: [0., 0., 0., 1.],
            },
        },
    }
    .into()
}

async fn exercise(extra_mount: bool) -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let owner = pair.b.peer_id().to_string();
        let home_owner = pair.a.peer_id().to_string();
        let temp = tempfile::tempdir()?;
        let homes = support::seeded_local_scenes(temp.path())?;
        let home_frame = spatial_home::conventions::registry(&home_owner,
            &homes.snapshot("humanoid-hub-conference")?.scenegraph.map.frame.id)?;
        let frame = if extra_mount {Frame::ros_optical(&owner,"lens")} else {Frame::opengl(&owner,"lens")};
        let base = Frame::opengl(&owner,"platform");
        let world = Frame::opengl(&owner,"epoch-1");
        let runtime = ComponentRuntime::new(owner.clone());
        let endpoint = auki_component_protocol::ComponentProtocolEndpoint::mount(pair.b.protocols(), runtime.clone())?;
        let defs = registry::Definitions::default();
        for f in [&frame,&base,&world] {registry::frame(&defs,f)?;}
        let clock = spatial_home::model::clock_entry(&owner,"s");
        registry::clock(&defs,&clock)?;
        let clock = spatial_home::model::clock_ref(&clock);
        let sensor: auki_registry::SensorRegistryEntry = serde_json::from_value(json!({
            "peer_id":owner,"sensor_id":"lens-definition","kind":"camera","type":"mono",
            "width":100,"height":80,"frame_rate_hz":5,"image_encoding":"jpeg","pixel_format":"gray8",
            "row_stride_bytes":0,"color_space":"gray","intrinsics_model":"pinhole","distortion_model":"none",
            "calibration":{"fx":100.,"fy":100.,"cx":50.,"cy":40.},
            "frame":{"peer_id":owner,"id":frame.frame_id,"hash":frame.hash()}}))?;
        registry::insert(&defs, auki_protocols::registry::v3::RegistryEntryEnvelope {
            kind:auki_protocols::registry::v3::RegistryKind::Sensor,id:sensor.sensor_id.clone(),hash:sensor.hash(),
            canonical_json:String::from_utf8(sensor.canonical_bytes())?})?;
        let _registry = registry::mount(&pair.b,runtime.clone(),defs)?;
        let home_defs = registry::Definitions::default();
        registry::frame(&home_defs,&home_frame)?;
        let c = registry_ar::CameraContract {sensor:auki_registry::RegistryRef {peer_id:owner.clone(),id:sensor.sensor_id.clone(),hash:sensor.hash()},
            frame:(&frame).into(),clock:clock.clone(),session_id:"s".into()};
        let component = runtime.component(ComponentSpec::new("image-source").observable(ObservableContract {
            name:"pixels".into(),datatype:VideoFrame::DATATYPE.into(),schema:"auki.video-frame/v1".into(),
            access:vec![ObservationAccess::FollowNew],exposure:Exposure::Cluster }))?;
        let output = component.configured_observable(ConfiguredObservableSpec::new("pixels",format!("camera-{}",manifest_hash(&c)),clock.clone(),
            PayloadContract::Camera(CameraPayloadContract {datatype:VideoFrame::DATATYPE.into(),schema:"auki.video-frame/v1".into(),encoding:"jpeg".into(),
                width:100,height:80,nominal_frame_rate_hz:Some(5),observes:"test image".into()})).in_registered_frame(frame.clone()))?;
        component.expose()?;
        let capture = runtime.capture_buffer_with_metadata("arbitrary-image-product",&output,
            BufferLimits {max_entries:Some(2),max_bytes:None,target_duration:None},|f:&VideoFrame|f.bytes.len(),
            move |o| Ok(Some(CatalogProductMetadata {schema:registry_ar::CAMERA_METADATA.into(),source_sequence:o.sequence,value:serde_json::to_value(&c).map_err(|e| e.to_string())?})))?;
        endpoint.export_product(&capture.product())?;
        let t = 9_007_199_254_740_993;
        let _mount = if extra_mount {
            let m = transform(&runtime,&endpoint,&frame,&base,&clock,Lifetime::Held,"mount")?;
            m.1.publish(0,valid(1.))?; Some(m)
        } else {None};
        let (_motion_capture,motion) = transform(&runtime,&endpoint,if extra_mount {&base} else {&frame},&world,&clock,Lifetime::Sampled,"motion")?;
        motion.publish(t-50_000_000,valid(2.))?;
        let (_alignment_capture,alignment) = transform(&runtime,&endpoint,&world,&home_frame,&clock,Lifetime::Held,"location")?;
        alignment.publish(0,valid(10.))?;
        let jpeg = {
            let mut bytes=Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut bytes).encode(&vec![0;100*80],100,80,image::ExtendedColorType::L8)?;
            bytes
        };
        let (image,_) = output.publish(t,VideoFrame {width:100,height:80,encoding:"jpeg".into(),bytes:jpeg.into()}.into())?;
        let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
        let mut row = spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
        row["peer_id"]=json!(owner);
        row["registries"] = registry::resolve(&pair.a,pair.b.peer_id(),&row,None,&home_defs).await;
        assert_eq!(row["registries"]["status"],"ready","{row}");
        let cameras = spatial_home::sensor_preview::camera_sensors(&std::collections::BTreeMap::from([(owner.clone(),row.clone())]));
        assert_eq!(cameras.len(),1);
        let source = &cameras[0];
        let client = auki_component_protocol::ComponentProtocolClient::new(pair.a.protocols());
        let manifest = output.manifest();
        let render = || registry_ar::render(&client,source,&row,&image,&manifest,&homes,&home_owner,true);
        assert!(render().await.unwrap_err().to_string().contains("Waiting for capture-matched"));
        motion.publish(t+50_000_000,valid(4.))?;
        let view = render().await?;
        assert_eq!(view["capture_timestamp_ns"],t.to_string());
        assert_eq!(view["picking"]["camera_to_map"]["translation"],json!([if extra_mount {14.}else{13.},0.,0.]));
        assert_eq!(view["picking"]["frame"],home_frame.frame_id);
        assert_eq!(view["viewpoint"]["position"],view["picking"]["camera_to_map"]["translation"]);
        let network=json!({"peers":[row.clone()],"aliases":{owner.clone():"Arbitrary peer"}});
        let viewer=serde_json::from_value(json!({"frame":home_frame.frame_id,"mode":"3d","screen_right":[1,0,0]}))?;
        let action=spatial_home::view_camera::attach_action(&network,json!({"peer_id":owner,"frame_id":frame.frame_id,"product_id":"arbitrary-image-product"}),"local:humanoid-hub-conference",&home_frame.frame_id,Some(&viewer));
        // Camera and pose Products may describe the same frame; explicit product selection is exact.
        let action=action?;
        assert_eq!(action["source_frame"],view["source_frame"]);
        let ray=spatial_home::chat_camera::ray(&view,Some([50.,40.]),&home_frame.frame_id)?;
        assert_eq!(ray["origin"],view["picking"]["camera_to_map"]["translation"]);
        let optical: Pose = serde_json::from_value(view["picking"]["camera_to_map"].clone())?;
        let projected = spatial_home::ar_preview::project_view(&json!({
            "frame":home_frame.frame_id,"voxel_size":0.1,"voxels":[],
            "notes":[{"id":"target","text":"target","position":[if extra_mount {14.}else{13.},0.,if extra_mount {2.}else{-2.}]}]
        }), &optical, &view["calibration"])?;
        assert_eq!(projected["notes"][0]["pixel"],json!([50.,40.]));
        // Latest calibration from the future must not replace capture-time state.
        alignment.publish(t+1,valid(999.))?;
        assert_eq!(render().await?["picking"],view["picking"]);
        // Changed immutable descriptor, wrong clock, or missing exact Registry fail closed.
        let mut bad = row.clone();
        bad["registries"]["definitions"] = json!({});
        assert!(registry_ar::render(&client,source,&bad,&image,&manifest,&homes,&home_owner,true).await.is_err());
        let mut wrong = manifest.clone(); wrong.clock.hash = "b".repeat(32);
        assert!(registry_ar::render(&client,source,&row,&image,&wrong,&homes,&home_owner,true).await.is_err());
        let mut mutated = row.clone();
        for entry in mutated["registries"]["definitions"].as_object_mut().unwrap().values_mut() {
            if entry["kind"] == "camera" { entry["calibration"]["fx"] = json!(999.); }
        }
        assert!(registry_ar::render(&client,source,&mutated,&image,&manifest,&homes,&home_owner,true).await.is_err());
        let mut descriptor = row.clone();
        for product in descriptor["catalog"]["products"].as_array_mut().unwrap() {
            if product["metadata"]["schema"] == registry_ar::CAMERA_METADATA {
                product["metadata"]["value"]["session_id"] = json!("changed");
            }
        }
        assert!(registry_ar::render(&client,source,&descriptor,&image,&manifest,&homes,&home_owner,true).await.is_err());
        // A second declared route is ambiguous, even when both yield the same pose.
        let (_duplicate_capture,duplicate) = transform(&runtime,&endpoint,&world,&home_frame,&clock,Lifetime::Held,"alternative-location")?;
        duplicate.publish(0,valid(10.))?;
        let mut ambiguous = row.clone(); ambiguous["catalog"] = serde_json::to_value(runtime.catalog().snapshot())?;
        assert!(registry_ar::render(&client,source,&ambiguous,&image,&manifest,&homes,&home_owner,true).await.unwrap_err().to_string().contains("unambiguous"));
        // Revocation cannot be replaced with an earlier valid alignment.
        alignment.publish(t+2,Update {tracking:Tracking::Lost}.into())?;
        let (later,_) = output.publish(t+3,image.payload.clone())?;
        motion.publish(t+100_000_000,valid(4.))?;
        assert!(registry_ar::render(&client,source,&row,&later,&manifest,&homes,&home_owner,true).await.unwrap_err().to_string().contains("revoked"));
        Ok::<(),anyhow::Error>(())
    }.await;
    pair.shutdown().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_edge_camera_chain_over_authenticated_sdk() -> Result<()> {
    exercise(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_edge_camera_chain_over_authenticated_sdk() -> Result<()> {
    exercise(true).await
}
