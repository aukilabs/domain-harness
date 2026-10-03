//! Read-only Galbot RGB-D Component. Sensor capture runs beside the independent
//! mapper; Home aligns each observation then distributes it to both mappers.
use crate::{
    network::HomeConnection,
    runner_voxels::{PreparedScan, SensorCalibration},
};
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::*;
use auki_components::*;
use auki_registry::FrameRegistryEntry;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
pub const FRAME: &str = "right_arm_camera_color_optical_frame";
pub fn supported_camera_frame(frame: &str) -> bool {
    matches!(frame, FRAME | "left_arm_camera_color_optical_frame")
}
pub const PRODUCT: &str = "rgbd.right_arm-history";
pub const SCHEMA: &str = "spatial-harness.rgbd/v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub timestamp_ns: u64,
    pub rgb_timestamp_ns: u64,
    pub transform_timestamp_ns: u64,
    pub hit_points_m: Vec<[f64; 3]>,
    pub hit_colors_srgb: Vec<[u8; 3]>,
    pub calibration: SensorCalibration,
}
impl ContractType for Capture {
    const DATATYPE: &'static str = SCHEMA;
}
impl Capture {
    pub fn validate(&self) -> Result<()> {
        self.validate_in_frame(FRAME)
    }
    pub fn validate_in_frame(&self, expected_frame: &str) -> Result<()> {
        self.calibration.validate()?;
        ensure!(
            self.timestamp_ns > 0
                && self.timestamp_ns == self.rgb_timestamp_ns
                && self.timestamp_ns == self.transform_timestamp_ns,
            "RGB-D capture/pose timestamps differ"
        );
        ensure!(
            supported_camera_frame(expected_frame)
                && self.calibration.sensor_frame == expected_frame
                && self.calibration.calibration_source.starts_with(
                    "Galbot SDK D405 co-registered RGB-D; RealSense deprojection; sha256:"
                ),
            "unverified RGB-D calibration"
        );
        ensure!(
            !self.hit_points_m.is_empty()
                && self.hit_points_m.len() <= 384
                && self.hit_points_m.len() == self.hit_colors_srgb.len(),
            "invalid RGB-D point/color count"
        );
        ensure!(
            self.hit_points_m
                .iter()
                .all(|p| p.iter().all(|v| v.is_finite())
                    && (0.099..=0.501).contains(&p[2])
                    && p.iter().map(|v| v * v).sum::<f64>() < 1.),
            "RGB-D depth out of range"
        );
        Ok(())
    }
}
pub struct Publisher {
    output: ConfiguredObservable<Capture>,
    _component: Component,
    capture: BufferProductCapture<Capture>,
}
impl Publisher {
    pub fn mount(
        runtime: &ComponentRuntime,
        endpoint: &ComponentProtocolEndpoint,
        owner: &str,
        clock: ClockReference,
    ) -> Result<Self> {
        let component = runtime.component(ComponentSpec::new("rgbd.right_arm").observable(
            ObservableContract {
                name: "samples".into(),
                datatype: SCHEMA.into(),
                schema: SCHEMA.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            },
        ))?;
        let output = component.configured_observable(
            ConfiguredObservableSpec::new(
                "samples",
                format!("rgbd-{}", uuid::Uuid::new_v4()),
                clock,
                PayloadContract::Structured(StructuredPayloadContract {
                    modality: "colored_depth".into(),
                    datatype: SCHEMA.into(),
                    schema: SCHEMA.into(),
                    observes: "right_arm_rgbd".into(),
                    unit: Some("meters".into()),
                }),
            )
            .in_registered_frame(FrameRegistryEntry::ros_optical(owner, FRAME)),
        )?;
        component.expose()?;
        let capture = runtime.capture_buffer(PRODUCT, &output, BufferLimits::entries(3), size)?;
        endpoint.export_product(&capture.product())?;
        Ok(Self {
            output,
            _component: component,
            capture,
        })
    }
    pub fn publish(&self, sample: Capture) -> Result<()> {
        sample.validate()?;
        self.output.publish(sample.timestamp_ns, Arc::new(sample))?;
        ensure!(self.capture.errors().is_empty(), "RGB-D retention failed");
        Ok(())
    }
    pub async fn run(self, script: std::path::PathBuf) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut child = tokio::process::Command::new("python3.8")
            .arg(script).env("PYTHONPATH","/data/galbot/lib:/data/galbot/lib/python3.8.10:/data/galbot/lib/python3/site-packages")
            .env("LD_LIBRARY_PATH","/data/galbot/lib")
            // Camera code does not need transport credentials.
            .env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
            .stdout(std::process::Stdio::piped()).kill_on_drop(true).spawn()?;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(json) = line.strip_prefix("RGBD ") {
                ensure!(json.len() < 128 * 1024, "RGB-D capture too large");
                self.publish(serde_json::from_str(json)?)?;
            }
        }
        bail!("RGB-D capture exited: {}", child.wait().await?)
    }
}
fn size(sample: &Capture) -> usize {
    serde_json::to_vec(sample).map_or(usize::MAX, |s| s.len())
}

pub async fn fetch(
    client: &ComponentProtocolClient,
    connection: Option<&HomeConnection>,
) -> Result<RemoteObservations<Capture>> {
    let Some(connection) = connection else {
        std::future::pending::<()>().await;
        unreachable!()
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let expected = connection.peer_id.parse()?;
    let mut error = anyhow!("RGB-D route unavailable");
    for route in &connection.routes {
        let request = async {
            let CatalogResponse::Snapshot { snapshot } =
                client.catalog_exact(expected, route.parse()?, None).await?
            else {
                bail!("RGB-D catalog unavailable")
            };
            let product = snapshot
                .products
                .iter()
                .find(|p| p.manifest.product_id == PRODUCT)
                .ok_or_else(|| anyhow!("RGB-D Component not published yet"))?;
            Ok::<_, anyhow::Error>(
                client
                    .observations_exact(
                        expected,
                        route.parse()?,
                        ObservationRequest {
                            product: ProductReference {
                                peer_id: connection.peer_id.clone(),
                                product_id: PRODUCT.into(),
                                manifest_hash: product.manifest_hash.clone(),
                            },
                            selection: ObservationSelection::LatestExisting,
                        },
                    )
                    .await?,
            )
        };
        match tokio::time::timeout(Duration::from_secs(10), request).await {
            Ok(Ok(sample)) => return Ok(sample),
            Ok(Err(e)) => error = e,
            Err(e) => error = e.into(),
        }
    }
    Err(error)
}
pub fn place(
    sample: &RemoteObservations<Capture>,
    expected: &str,
    odom: &RetainedProduct<auki_odometry::PoseUpdate>,
    alignment: &crate::runner_pose::MapAlignment,
    alignment_time: u64,
) -> Result<Option<PreparedScan>> {
    ensure!(
        sample.gap.is_none() && sample.end.is_none(),
        "RGB-D stream gap/end"
    );
    let o = sample
        .observations
        .last()
        .ok_or_else(|| anyhow!("RGB-D has no observations yet"))?;
    o.payload.validate()?;
    let clock = &alignment.transform.contract.clock;
    let frame = FrameRegistryEntry::ros_optical(expected, FRAME);
    ensure!(
        sample.product.peer_id == expected
            && sample.product.product_id == PRODUCT
            && sample.producer.peer_id == expected
            && sample.producer.component_id == "rgbd.right_arm"
            && sample.producer.payload.schema() == SCHEMA
            && &sample.producer.clock == clock
            && sample
                .producer
                .spatial_frame
                .as_ref()
                .map(|f| &f.definition)
                == Some(&frame)
            && o.output == sample.producer.reference(),
        "RGB-D producer/frame/clock mismatch"
    );
    ensure!(
        o.timestamp_ns == o.payload.timestamp_ns && o.timestamp_ns >= alignment_time,
        "RGB-D predates alignment or timestamp mismatch"
    );
    let Some(base) = crate::runner_voxels::capture_pose(odom, alignment, o.timestamp_ns)? else {
        return Ok(None);
    };
    Ok(Some(PreparedScan {
        source: auki_voxel_map::SensorBinding {
            sensor_product: ProductReference {
                peer_id: expected.into(),
                product_id: PRODUCT.into(),
                manifest_hash: sample.product_manifest_hash.clone(),
            },
            sensor_frame: frame,
            observation_clock: clock.clone(),
        },
        sequence: o.sequence,
        timestamp_ns: o.timestamp_ns,
        hit_points_m: o.payload.hit_points_m.clone(),
        hit_colors_srgb: o
            .payload
            .hit_colors_srgb
            .iter()
            .copied()
            .map(Some)
            .collect(),
        sensor_to_home: crate::runner_pose::compose(&base, &o.payload.calibration.sensor_to_base),
        alignment_key: serde_json::to_string(alignment)?,
        calibration: o.payload.calibration.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Capture {
        Capture {
            timestamp_ns: 10,
            rgb_timestamp_ns: 10,
            transform_timestamp_ns: 10,
            hit_points_m: vec![[0., 0., 0.3]],
            hit_colors_srgb: vec![[10, 80, 230]],
            calibration: SensorCalibration {
                sensor_frame: FRAME.into(),
                sensor_to_base: auki_odometry::Pose {
                    translation: [0.2, 0., 0.6],
                    rotation_xyzw: [0., 0., 0., 1.],
                },
                calibration_source:
                    "Galbot SDK D405 co-registered RGB-D; RealSense deprojection; sha256:fixture"
                        .into(),
            },
        }
    }
    #[test]
    fn validates_capture_time_color_pairing_and_measured_range() {
        let good = sample();
        good.validate().unwrap();
        for n in 0..5 {
            let mut bad = good.clone();
            match n {
                0 => bad.rgb_timestamp_ns += 1,
                1 => bad.transform_timestamp_ns += 1,
                2 => bad.hit_colors_srgb.clear(),
                3 => bad.hit_points_m[0][2] = 0.,
                _ => bad.hit_points_m[0][0] = f64::NAN,
            }
            assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn articulated_camera_and_lidar_coexist_and_persist_without_duplicate_evidence() -> Result<()> {
        use crate::model::Store;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("map.json");
        let mut store = Store::open_live(&path)?;
        let capture = sample();
        let clock = crate::model::test_clock("physical-robot");
        let mut input = PreparedScan {
            source: auki_voxel_map::SensorBinding {
                sensor_product: ProductReference {
                    peer_id: "robot".into(),
                    product_id: PRODUCT.into(),
                    manifest_hash: "one".into(),
                },
                sensor_frame: FrameRegistryEntry::ros_optical("robot", FRAME),
                observation_clock: clock,
            },
            sequence: 1,
            timestamp_ns: 10,
            hit_points_m: capture.hit_points_m.clone(),
            hit_colors_srgb: vec![Some([10, 80, 230])],
            sensor_to_home: capture.calibration.sensor_to_base.clone(),
            calibration: capture.calibration,
            alignment_key: "same".into(),
        };
        let mut lidar = input.clone();
        lidar.source.sensor_product.product_id = "lidar.base-history".into();
        lidar.source.sensor_frame = FrameRegistryEntry::ros_body("robot", "lidar_base_link");
        lidar.calibration.sensor_frame = "lidar_base_link".into();
        lidar.calibration.calibration_source = "fixed lidar fixture".into();
        lidar.hit_colors_srgb.clear();
        store.integrate_prepared(lidar.clone())?;
        store.integrate_prepared(input.clone())?;
        input.sequence += 1;
        input.timestamp_ns += 1;
        input.calibration.sensor_to_base.translation[0] += 0.1;
        input.sensor_to_home = input.calibration.sensor_to_base.clone();
        store.integrate_prepared(input.clone())?;
        assert_eq!(store.built.voxels.definition.sources.len(), 2);
        assert!(
            store
                .built
                .view
                .voxels
                .iter()
                .any(|v| v.linear_rgb.is_some())
        );
        let voxels = store.built.voxels.clone();
        drop(store);
        let mut restored = Store::open_live(path)?;
        assert_eq!(restored.built.voxels, voxels);
        assert!(!restored.integrate_prepared(input.clone())?);
        input.sequence += 1;
        input.timestamp_ns += 1;
        input.calibration.calibration_source.push_str(" changed");
        assert!(restored.integrate_prepared(input).is_err());
        lidar.sequence += 1;
        lidar.timestamp_ns += 1;
        lidar.calibration.sensor_to_base.translation[0] += 0.1;
        assert!(restored.integrate_prepared(lidar).is_err());
        assert_eq!(restored.built.voxels, voxels);
        Ok(())
    }
}
