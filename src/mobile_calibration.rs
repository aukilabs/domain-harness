//! Pixel-only phone calibration using the SDK QR detector and square pose solver.
//! The caller supplies a reference from the authenticated selected Home Product.
//! Camera pixels, intrinsics and AR pose must describe the same captured frame.
use crate::local_mapper::{compose, from_rigid, inverse, rigid};
use anyhow::{Result, ensure};
use auki_geometry::pnp::{Camera, Vector2, Vector3, estimate_square_pose_from_pixels, pose_tools};
use auki_scenegraph::{RigidTransform, Scenegraph};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, time::Instant};

thread_local! {
    // Each capture worker retains its SDK scanner; no poses or Home references
    // are cached here. Keep single-frame mode so old pixels cannot align a new scene.
    static DETECTOR: RefCell<auki_qr_detector::QrDetector> =
        RefCell::new(auki_qr_detector::QrDetector::new(Default::default()));
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub session: String,
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub camera_to_session: RigidTransform,
    pub scenegraph: Scenegraph,
}
#[derive(Serialize)]
pub struct Report {
    pub detail: String,
    pub alignment: Option<crate::mobile_depth::Alignment>,
    pub origin_observation: Option<crate::scene_origin::Observation>,
    pub diagnostics: DetectionDiagnostics,
}

/// Local diagnostics only: no pixels, decoded payloads, credentials or poses.
#[derive(Default, Serialize)]
pub struct DetectionDiagnostics {
    pub validation_us: u64,
    pub decode_us: u64,
    pub pose_us: u64,
    pub total_us: u64,
    pub decoded: usize,
    pub matched: usize,
    pub mirrored: usize,
    pub pose_rejection: Option<String>,
}

impl Capture {
    fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.session)?;
        ensure!(
            self.timestamp_ns > 0
                && self.width > 0
                && self.height > 0
                && self.width <= 1920
                && self.height <= 1920,
            "Invalid capture dimensions/time"
        );
        ensure!(
            [self.fx, self.fy, self.cx, self.cy]
                .iter()
                .all(|v| v.is_finite())
                && self.fx > 0.
                && self.fy > 0.,
            "Invalid camera intrinsics"
        );
        ensure!(
            self.camera_to_session.from_frame_id == format!("phone-camera:{}", self.session)
                && self.camera_to_session.to_frame_id == format!("phone-ar:{}", self.session),
            "Wrong capture frame"
        );
        let p = &self.camera_to_session;
        ensure!(
            p.translation
                .iter()
                .chain(p.rotation_wxyz.iter())
                .all(|v| v.is_finite())
                && (p.rotation_wxyz.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 0.001,
            "Invalid AR pose"
        );
        crate::conventions::validate_frame(&self.scenegraph.map.frame)?;
        self.scenegraph.validate()?;
        ensure!(
            self.scenegraph.map.frame.convention
                == Some(auki_registry::CoordinateConvention::OpenGl),
            "Expected OpenGL map"
        );
        ensure!(
            self.scenegraph.map.frame.meters_per_unit == 1.
                && self.scenegraph.anchors.len() <= 1024,
            "Invalid Portal reference"
        );
        Ok(())
    }
}

pub fn detect(luma: &[u8], capture: &Capture) -> Result<Report> {
    let started = Instant::now();
    capture.validate()?;
    ensure!(
        luma.len() == capture.width * capture.height,
        "Invalid packed luma image"
    );
    let validation_us = started.elapsed().as_micros() as u64;
    let decoding = Instant::now();
    let codes = DETECTOR.with(|detector| {
        detector
            .borrow_mut()
            .detect_luma(luma, capture.width, capture.height, capture.width)
    })?;
    let mut report = Report {
        detail: "Show a Portal from the selected Home scene".into(),
        alignment: None,
        origin_observation: None,
        diagnostics: DetectionDiagnostics {
            validation_us,
            decode_us: decoding.elapsed().as_micros() as u64,
            decoded: codes.codes.len(),
            mirrored: codes.codes.iter().filter(|c| c.mirrored).count(),
            ..Default::default()
        },
    };
    for code in codes.codes.iter().filter(|c| !c.mirrored) {
        if capture.scenegraph.anchors.is_empty() {
            let Some(id)=crate::local_mapper::portal_code(&code.payload) else {continue};
            let expected_scene=auki_scenegraph::MapSnapshot::new(capture.scenegraph.clone())?;
            let mut provisional=Capture{session:capture.session.clone(),timestamp_ns:capture.timestamp_ns,width:capture.width,height:capture.height,fx:capture.fx,fy:capture.fy,cx:capture.cx,cy:capture.cy,camera_to_session:capture.camera_to_session.clone(),scenegraph:capture.scenegraph.clone()};
            let anchor=crate::scene_origin::floor_anchor(id,crate::mobile_portal::DEFAULT_SIDE_M,&capture.scenegraph.map.frame.id)?;
            provisional.scenegraph.anchors.insert(id.into(),anchor);
            let corners=code.refined_corners_px.as_ref().unwrap_or(&code.corners_px).map(|p|[p.x,p.y]);
            if let Ok(measured)=align(&provisional,id,corners) {
                report.origin_observation=Some(crate::scene_origin::Observation{expected_scene,code:id.into(),side_length_m:crate::mobile_portal::DEFAULT_SIDE_M,timestamp_ns:capture.timestamp_ns,portal_to_native:measured.portal_to_session});
                report.detail=format!("Observed {id}; waiting for Home to accept the origin");break;
            }
            continue;
        }
        let Some(anchor) = capture
            .scenegraph
            .anchors
            .values()
            .find(|a| a.payload == code.payload)
            .or_else(|| {
                crate::local_mapper::portal_code(&code.payload)
                    .and_then(|id| capture.scenegraph.anchors.get(id))
            })
        else {
            report.detail = "QR detected, but not a Portal in the selected Home scene".into();
            continue;
        };
        let corners = code
            .refined_corners_px
            .as_ref()
            .unwrap_or(&code.corners_px)
            .map(|p| [p.x, p.y]);
        report.diagnostics.matched += 1;
        let solving = Instant::now();
        let result = align(capture, &anchor.anchor_id, corners);
        report.diagnostics.pose_us += solving.elapsed().as_micros() as u64;
        match result {
            Ok(alignment) => {
                report.detail = format!("Aligned with {}", anchor.anchor_id);
                report.alignment = Some(alignment);
                break;
            }
            Err(error) => {
                report.diagnostics.pose_rejection = Some(error.to_string());
                report.detail = format!(
                    "Recognized {} · move closer and hold steady",
                    anchor.anchor_id
                )
            }
        }
    }
    report.diagnostics.total_us = started.elapsed().as_micros() as u64;
    Ok(report)
}

pub fn align(
    c: &Capture,
    id: &str,
    corners: [[f64; 2]; 4],
) -> Result<crate::mobile_depth::Alignment> {
    c.validate()?;
    let anchor = c
        .scenegraph
        .anchors
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("Unknown Portal"))?;
    anchor.validate_in_map(&c.scenegraph.map)?;
    let mut area = 0.;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let d = corners[(i + 2) % 4];
        ensure!(
            a.iter().all(|v| v.is_finite())
                && a[0] >= 0.
                && a[1] >= 0.
                && a[0] < c.width as f64
                && a[1] < c.height as f64
                && (b[0] - a[0]) * (d[1] - b[1]) - (b[1] - a[1]) * (d[0] - b[0]) > 1e-9,
            "Invalid printed corner order"
        );
        area += a[0] * b[1] - a[1] * b[0];
    }
    ensure!(area / 2. >= 64., "Portal too small");
    let camera =
        Camera::pinhole(c.fx, c.fy, c.cx, c.cy).map_err(|_| anyhow::anyhow!("Invalid camera"))?;
    // SDK returns marker -> OpenGL camera, the same axes used by ARCore Pose.
    let estimate = estimate_square_pose_from_pixels(
        corners.map(|p| Vector2::new(p[0], p[1])),
        anchor.side_length_m,
        &camera,
    )
    .map_err(|_| anyhow::anyhow!("Pose solver failed"))?;
    let optical = pose_tools::from_opengl_to_opencv(&estimate.pose);
    let rotation = optical.rotation.to_na_unit();
    let t = optical.position.to_na();
    ensure!(
        t.z >= 0.1 && t.norm() <= 8. && (rotation * Vector3::new(0., 0., 1.).to_na()).dot(&t) < 0.,
        "Back-facing or distant Portal"
    );
    let h = anchor.side_length_m / 2.;
    let mut error = 0.;
    for ([x, y], observed) in [[-h, h], [h, h], [h, -h], [-h, -h]]
        .into_iter()
        .zip(corners)
    {
        let p = rotation * Vector3::new(x, y, 0.).to_na() + t;
        ensure!(p.z > 0., "Portal behind camera");
        let uv = camera
            .project(Vector3::from_na(&p))
            .ok_or_else(|| anyhow::anyhow!("Invalid projection"))?;
        error += (uv.x - observed[0]).powi(2) + (uv.y - observed[1]).powi(2);
    }
    ensure!((error / 4.).sqrt() <= 2., "Unstable Portal pose");
    let p = estimate.pose;
    let camera_from_portal = auki_odometry::Pose {
        translation: [p.position.x, p.position.y, p.position.z],
        rotation_xyzw: [p.rotation.x, p.rotation.y, p.rotation.z, p.rotation.w],
    };
    let session_from_portal = compose(&from_rigid(&c.camera_to_session), &camera_from_portal)?;
    let home_from_session = compose(
        &from_rigid(&anchor.pose_in_map),
        &inverse(&session_from_portal)?,
    )?;
    Ok(crate::mobile_depth::Alignment {
        timestamp_ns: c.timestamp_ns,
        portal_id: id.into(),
        portal_to_session: rigid(
            &session_from_portal,
            &anchor.pose_in_map.from_frame_id,
            &c.camera_to_session.to_frame_id,
        ),
        session_to_home: rigid(
            &home_from_session,
            &c.camera_to_session.to_frame_id,
            &c.scenegraph.map.frame.id,
        ),
    })
}
