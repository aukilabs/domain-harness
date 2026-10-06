//! Capture-matched AR: never project a latest robot pose over an older image.
use anyhow::{Result, anyhow, ensure};
use auki_components::{ClockReference, ProductReference};
use auki_odometry::Pose;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Frame {
    pub source: ProductReference,
    pub clock: ClockReference,
    pub timestamp_ns: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub intrinsics: [f64; 4],
    pub gray_base64: String,
    pub camera_to_map: Pose,
    pub origin: crate::local_mapper::Origin,
    pub received: Instant,
}
#[derive(Clone)]
pub struct PoseHistory {
    pub product: auki_components::RetainedProduct<auki_odometry::PoseUpdate>,
    pub contract: auki_odometry::PoseContract,
}
#[derive(Default)]
pub struct Cache {
    pub frames: BTreeMap<String, Frame>,
    pub poses: BTreeMap<String, PoseHistory>,
    pub alignments: BTreeMap<String, VecDeque<crate::local_mapper::Origin>>,
}
impl Cache {
    pub fn record_alignment(&mut self, peer: &str, origin: &crate::local_mapper::Origin) {
        let history = self.alignments.entry(peer.into()).or_default();
        if history
            .back()
            .is_some_and(|o| o.capture_clock != origin.capture_clock)
        {
            history.clear();
        }
        if history
            .back()
            .is_none_or(|o| o.capture_timestamp_ns < origin.capture_timestamp_ns)
        {
            if history.len() == 64 {
                history.pop_front();
            }
            history.push_back(origin.clone());
        }
    }
    pub fn alignment_at(
        &self,
        peer: &str,
        timestamp: u64,
        current: &crate::local_mapper::Origin,
    ) -> Result<crate::local_mapper::Origin> {
        if timestamp >= current.capture_timestamp_ns {
            return Ok(current.clone());
        }
        self.alignments
            .get(peer)
            .and_then(|h| {
                h.iter().rev().find(|o| {
                    o.capture_clock == current.capture_clock && o.capture_timestamp_ns <= timestamp
                })
            })
            .cloned()
            .ok_or_else(|| anyhow!("Waiting for calibration valid at camera capture time"))
    }
}
pub type Frames = Arc<Mutex<Cache>>;

pub fn render(frame: &Frame, scene: &crate::network::CanonicalScene) -> Result<Value> {
    ensure!(
        frame.received.elapsed() < Duration::from_secs(5),
        "Waiting for a fresh calibrated camera frame"
    );
    ensure!(
        frame.source.product_id == "local-camera-observations"
            && frame.source.peer_id == frame.clock.peer_id,
        "AR camera provenance mismatch"
    );
    ensure!(
        frame.intrinsics.iter().all(|v| v.is_finite())
            && frame.intrinsics[0] > 0.
            && frame.intrinsics[1] > 0.,
        "Invalid AR intrinsics"
    );
    let peer = &scene.registries["local_mapping"]["runners"][&frame.source.peer_id];
    ensure!(
        peer["status"] == "aligned",
        "Waiting for this camera's robot to calibrate into Home"
    );
    ensure!(
        frame.origin.kind == "canonical_portals"
            && serde_json::to_value(&frame.origin)? == peer["origin"],
        "Camera alignment changed; waiting for matching Home map"
    );
    ensure!(
        frame.clock == frame.origin.capture_clock
            && frame.timestamp_ns >= frame.origin.capture_timestamp_ns,
        "Camera clock or alignment epoch mismatch"
    );
    ensure!(
        scene
            .voxels
            .definition
            .sources
            .iter()
            .any(|s| s.sensor_product == frame.source && s.observation_clock == frame.clock),
        "Camera Product has not contributed to this Home map"
    );
    // Home's per-peer processors all solve into the canonical Z-up basis.
    // Their Origin equality above binds this sample to this shared session.
    let target_id = crate::local_mapper::display_frame(&scene.frame)?;
    let target = crate::conventions::registry(&scene.authority_peer_id, &target_id)?;
    let camera = crate::conventions::pose(
        &frame.camera_to_map,
        &scene.voxels.definition.frame,
        &target,
    )?;
    let mut voxels = crate::model::occupied(&scene.voxels)?;
    for voxel in &mut voxels {
        voxel.center =
            crate::conventions::point(voxel.center, &scene.voxels.definition.frame, &target)?;
    }
    let [fx, fy, cx, cy] = frame.intrinsics;
    let projected = project_scene(
        scene,
        &frame.camera_to_map,
        &json!({"K":[fx,0.,cx,0.,fy,cy,0.,0.,1.],"D":[],"distortion_model":"pinhole","width":frame.width,"height":frame.height}),
        false,
    )?;
    Ok(
        json!({"sequence":frame.sequence,"capture_timestamp_ns":frame.timestamp_ns,"clock":frame.clock,
        "width":frame.width,"height":frame.height,"encoding":"luma8","data":frame.gray_base64,
        "ar":{"status":"ready","notes":projected["notes"],"frame":target_id,"camera_to_map":camera,
        "intrinsics":{"fx":frame.intrinsics[0],"fy":frame.intrinsics[1],"cx":frame.intrinsics[2],"cy":frame.intrinsics[3]},
        "voxel_size":scene.voxels.definition.grid().voxel_size_m.0,"voxels":voxels}}),
    )
}

pub fn preview(
    sensor: &str,
    frames: &Frames,
    scene: Option<&crate::network::CanonicalScene>,
) -> Result<Value> {
    let (peer, product) = sensor
        .split_once('/')
        .ok_or_else(|| anyhow!("Unknown camera"))?;
    ensure!(
        product == "local-camera-history",
        "AR requires a calibrated local-camera-history feed"
    );
    let frame = frames
        .lock()
        .map_err(|_| anyhow!("AR frame cache unavailable"))?
        .frames
        .get(peer)
        .cloned()
        .ok_or_else(|| anyhow!("Waiting for Home to localize this camera"))?;
    render(
        &frame,
        scene.ok_or_else(|| anyhow!("Waiting for Home's collaborative live map"))?,
    )
}

/// SDK Brown projection plus the RealSense inverse-Brown tangential correction.
/// Vendor rs2_project_point_to_pixel uses radially modified x/y in tangential
/// terms for this model. The pinned SDK Brown model uses the original x/y.
/// Verified against the installed robot pyrealsense2 with off-axis golden points.
pub fn project_camera_point(
    camera: &auki_geometry::pnp::Camera,
    lens: &str,
    point: auki_geometry::pnp::Vector3,
) -> Option<auki_geometry::pnp::Vector2> {
    let mut pixel = camera.project(point)?;
    if lens == "Inverse Brown Conrady" {
        if camera.dist.len() != 5 {
            return None;
        }
        let x = point.x / point.z;
        let y = point.y / point.z;
        let r2 = x * x + y * y;
        let d = &camera.dist;
        let f = 1. + d[0] * r2 + d[1] * r2 * r2 + d[4] * r2 * r2 * r2;
        pixel.x += camera.fx * (f * f - 1.) * (2. * d[2] * x * y + 2. * d[3] * x * x);
        pixel.y += camera.fy * (f * f - 1.) * (2. * d[3] * x * y + 2. * d[2] * y * y);
    }
    (pixel.x.is_finite() && pixel.y.is_finite()).then_some(pixel)
}

pub fn camera_model(calibration: &Value) -> Result<auki_geometry::pnp::Camera> {
    use auki_geometry::pnp::Camera;
    let k = calibration["K"]
        .as_array()
        .ok_or_else(|| anyhow!("Camera matrix missing"))?;
    ensure!(k.len() == 9, "Expected a 3x3 camera matrix");
    let k: Vec<f64> = k
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| anyhow!("Invalid camera matrix"))
        })
        .collect::<Result<_>>()?;
    let d: Vec<f64> = calibration["D"]
        .as_array()
        .ok_or_else(|| anyhow!("Distortion coefficients missing"))?
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| anyhow!("Invalid lens coefficient"))
        })
        .collect::<Result<_>>()?;
    let lens = calibration["distortion_model"].as_str().unwrap_or("");
    let camera = match lens {
        "kannala_brandt" | "equidistant" | "opencv_fisheye" => {
            Camera::opencv_fisheye(k[0], k[4], k[2], k[5], &d)
        }
        "plumb_bob" | "rational_polynomial" | "Brown Conrady" | "brown_conrady" => {
            Camera::new(k[0], k[4], k[2], k[5], &d)
        }
        "none" | "pinhole" if d.iter().all(|v| *v == 0.) => Camera::pinhole(k[0], k[4], k[2], k[5]),
        "Inverse Brown Conrady" if d.len() == 5 => Camera::new(k[0], k[4], k[2], k[5], &d),
        _ => anyhow::bail!("Unsupported AR lens model: {lens}"),
    }
    .map_err(|e| anyhow!("Camera calibration: {e:?}"))?;
    Ok(camera)
}

/// Invert the same SDK/vendor projection used for rendering, including fisheye.
pub fn pixel_ray(calibration: &Value, pixel: [f64; 2]) -> Result<[f64; 3]> {
    use auki_geometry::pnp::{Vector2, Vector3};
    ensure!(pixel.iter().all(|x| x.is_finite()), "Invalid camera pixel");
    let camera = camera_model(calibration)?;
    let lens = calibration["distortion_model"].as_str().unwrap_or("");
    let ray = camera.unproject_opencv_ray(Vector2::new(pixel[0], pixel[1]));
    let (mut x, mut y) = (ray.direction.x, ray.direction.y);
    for _ in 0..12 {
        let p = project_camera_point(&camera, lens, Vector3::new(x, y, 1.))
            .ok_or_else(|| anyhow!("Pixel cannot be unprojected"))?;
        let (ex, ey) = (p.x - pixel[0], p.y - pixel[1]);
        if ex.hypot(ey) < 0.001 {
            return Ok([x, y, 1.]);
        }
        let h = 1e-6;
        let px = project_camera_point(&camera, lens, Vector3::new(x + h, y, 1.))
            .ok_or_else(|| anyhow!("Invalid camera ray"))?;
        let py = project_camera_point(&camera, lens, Vector3::new(x, y + h, 1.))
            .ok_or_else(|| anyhow!("Invalid camera ray"))?;
        let (a, b, c, d) = (
            (px.x - p.x) / h,
            (py.x - p.x) / h,
            (px.y - p.y) / h,
            (py.y - p.y) / h,
        );
        let det = a * d - b * c;
        ensure!(
            det.is_finite() && det.abs() > 1e-12,
            "Camera unprojection failed"
        );
        x -= (d * ex - b * ey) / det;
        y -= (-c * ex + a * ey) / det;
    }
    anyhow::bail!("Camera unprojection did not converge")
}

/// Project occupied cube surfaces with the SDK's measured camera model. The
/// browser receives pixels/depth, not another handwritten distortion model.
pub fn projected_voxels(
    scene: &crate::network::CanonicalScene,
    camera_to_map: &Pose,
    calibration: &Value,
) -> Result<Value> {
    project_scene(scene, camera_to_map, calibration, true)
}
fn project_scene(
    scene: &crate::network::CanonicalScene,
    camera_to_map: &Pose,
    calibration: &Value,
    include_voxels: bool,
) -> Result<Value> {
    use auki_geometry::pnp::Vector3;
    let camera = camera_model(calibration)?;
    let lens = calibration["distortion_model"].as_str().unwrap_or("");
    let p = crate::local_mapper::inverse(camera_to_map)?;
    let [x, y, z, w] = p.rotation_xyzw;
    let matrix =
        auki_geometry::spatial_transform_to_matrix4(&auki_datatypes::pose::SpatialTransform {
            translation: Some(auki_datatypes::pose::Vec3 {
                x: p.translation[0],
                y: p.translation[1],
                z: p.translation[2],
            }),
            orientation: Some(auki_datatypes::pose::Quat { x, y, z, w }),
        })?;
    let width = calibration["width"]
        .as_f64()
        .ok_or_else(|| anyhow!("Camera width missing"))?;
    let height = calibration["height"]
        .as_f64()
        .ok_or_else(|| anyhow!("Camera height missing"))?;
    let half = scene.voxels.definition.grid().voxel_size_m.0 / 2.;
    let faces = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let mut vertices: Vec<[f32; 6]> = Vec::new();
    let mut count = 0;
    for voxel in if include_voxels {
        crate::model::occupied(&scene.voxels)?
    } else {
        vec![]
    } {
        let mut corners = [[0.; 3]; 8];
        let mut visible = true;
        for (i, corner) in corners.iter_mut().enumerate() {
            let p: [f64; 3] = std::array::from_fn(|j| {
                voxel.center[j] + if i & (1 << j) == 0 { -half } else { half }
            });
            let c: [f64; 3] = std::array::from_fn(|r| {
                matrix[r][3] + (0..3).map(|j| matrix[r][j] * p[j]).sum::<f64>()
            });
            // Explicit positive near plane: SDK projection also accepts negative Z.
            if c[2] < 0.03 || c[2] > 40. {
                visible = false;
                break;
            }
            let Some(pixel) = project_camera_point(&camera, lens, Vector3::new(c[0], c[1], c[2]))
            else {
                visible = false;
                break;
            };
            *corner = [pixel.x, pixel.y, c[2]];
        }
        if !visible
            || corners.iter().all(|c| c[0] < 0.)
            || corners.iter().all(|c| c[0] > width)
            || corners.iter().all(|c| c[1] < 0.)
            || corners.iter().all(|c| c[1] > height)
        {
            continue;
        }
        let color = voxel.linear_rgb.unwrap_or([0.04, 0.65, 0.72]);
        for [a, b, c, d] in faces {
            for i in [a, b, c, a, c, d] {
                let p = corners[i];
                vertices.push([
                    p[0] as f32,
                    p[1] as f32,
                    p[2] as f32,
                    color[0] as f32,
                    color[1] as f32,
                    color[2] as f32,
                ]);
            }
        }
        count += 1;
    }
    let mut notes = Vec::new();
    for note in crate::notes::from_registries(&scene.registries, &scene.frame)?.items {
        let c: [f64; 3] = std::array::from_fn(|r| {
            matrix[r][3] + (0..3).map(|j| matrix[r][j] * note.position[j]).sum::<f64>()
        });
        if !(0.03..=40.).contains(&c[2]) {
            continue;
        }
        if let Some(pixel) = project_camera_point(&camera, lens, Vector3::new(c[0], c[1], c[2]))
            && (0. ..width).contains(&pixel.x)
            && (0. ..height).contains(&pixel.y)
        {
            notes.push(
                json!({"id":note.id,"text":note.text,"pixel":[pixel.x,pixel.y],"depth_m":c[2]}),
            );
        }
    }
    Ok(
        json!({"status":"ready","frame":scene.frame,"projection":"sdk-camera","lens_model":lens,"vertices":vertices,"voxel_count":count,"notes":notes}),
    )
}

#[allow(clippy::too_many_arguments)] // Exact image/pose manifests and camera-owned calibration are separate inputs.
pub fn camera_pose_preview(
    source: &ProductReference,
    image: &auki_components::Observation<auki_components::VideoFrame>,
    image_output: &auki_components::OutputManifest,
    geometry: &auki_components::Observation<auki_odometry::PoseUpdate>,
    geometry_output: &auki_components::OutputManifest,
    calibration: &Value,
    cache: &Frames,
    scene: &crate::network::CanonicalScene,
) -> Result<Value> {
    camera_pose_preview_mode(
        source,
        image,
        image_output,
        geometry,
        geometry_output,
        calibration,
        cache,
        scene,
        true,
    )
}

#[allow(clippy::too_many_arguments)] // Exact image/pose manifests and camera-owned calibration are separate inputs.
pub fn camera_pose_preview_mode(
    source: &ProductReference,
    image: &auki_components::Observation<auki_components::VideoFrame>,
    image_output: &auki_components::OutputManifest,
    geometry: &auki_components::Observation<auki_odometry::PoseUpdate>,
    geometry_output: &auki_components::OutputManifest,
    calibration: &Value,
    cache: &Frames,
    scene: &crate::network::CanonicalScene,
    overlay: bool,
) -> Result<Value> {
    use base64::Engine;
    let update = &geometry.payload;
    update.validate()?;
    let auki_odometry::Tracking::Valid {
        pose: sensor_to_base,
    } = &update.tracking
    else {
        anyhow::bail!("Sensor pose tracking unavailable")
    };
    let frame = image_output
        .spatial_frame
        .as_ref()
        .ok_or_else(|| anyhow!("Camera optical frame missing"))?;
    frame.validate().map_err(|e| anyhow!(e))?;
    ensure!(
        image_output.peer_id == source.peer_id
            && geometry_output.peer_id == source.peer_id
            && image.output == image_output.reference()
            && geometry.output == geometry_output.reference(),
        "Camera/geometry owner mismatch"
    );
    ensure!(
        update.contract.from_frame == frame.definition
            && geometry_output
                .spatial_frame
                .as_ref()
                .map(|f| &f.definition)
                == Some(&update.contract.to_frame),
        "Camera/pose frame mismatch"
    );
    ensure!(
        image.timestamp_ns == geometry.timestamp_ns
            && image_output.clock == geometry_output.clock
            && update.contract.clock == geometry_output.clock,
        "Camera/transform capture time or clock mismatch"
    );
    ensure!(
        calibration["width"].as_u64() == Some(image.payload.width as u64)
            && calibration["height"].as_u64() == Some(image.payload.height as u64),
        "Calibration dimensions differ from image"
    );
    sensor_to_base.validate()?;
    let local = &scene.registries["local_mapping"]["runners"][&source.peer_id];
    ensure!(
        local["status"] == "aligned",
        "Camera robot is not aligned to Home"
    );
    let origin: crate::local_mapper::Origin = serde_json::from_value(local["origin"].clone())?;
    let origin = cache
        .lock()
        .map_err(|_| anyhow!("AR cache unavailable"))?
        .alignment_at(&source.peer_id, image.timestamp_ns, &origin)?;
    ensure!(
        origin.kind == "canonical_portals"
            && origin.capture_clock == image_output.clock
            && image.timestamp_ns >= origin.capture_timestamp_ns,
        "Camera belongs to a different alignment session"
    );
    let history = cache
        .lock()
        .map_err(|_| anyhow!("AR cache unavailable"))?
        .poses
        .get(&source.peer_id)
        .cloned()
        .ok_or_else(|| anyhow!("Camera odometry unavailable"))?;
    ensure!(
        history.contract.clock == image_output.clock
            && history.contract.from_frame == update.contract.to_frame
            && history.contract.session_id == update.contract.session_id,
        "Camera and odometry clocks differ"
    );
    let newest = history
        .product
        .latest_existing()?
        .ok_or_else(|| anyhow!("Waiting for camera odometry"))?
        .timestamp_ns;
    ensure!(
        newest.saturating_sub(image.timestamp_ns) <= 3_000_000_000,
        "Camera frame is stale relative to robot odometry"
    );
    let base = auki_odometry::pose_at(
        &history.product,
        &auki_odometry::PoseQuery {
            contract: history.contract,
            timestamp_ns: image.timestamp_ns,
            max_gap_ns: 200_000_000,
        },
    )?;
    let optical = crate::local_mapper::compose(
        &origin.odometry_to_local,
        &crate::local_mapper::compose(&base.pose, sensor_to_base)?,
    )?;
    let ar = if overlay {
        projected_voxels(scene, &optical, calibration)?
    } else {
        Value::Null
    };
    Ok(
        json!({"sequence":image.sequence,"capture_timestamp_ns":image.timestamp_ns,"clock":image_output.clock,"spatial_frame":frame,"calibration":calibration,
        "picking":{"frame":scene.frame,"camera_to_map":optical,"calibration":calibration},"width":image.payload.width,"height":image.payload.height,"encoding":image.payload.encoding,"data":base64::engine::general_purpose::STANDARD.encode(&image.payload.bytes),"ar":ar}),
    )
}
