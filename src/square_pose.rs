//! A square of known side (a Portal or an onboarding marker) in the camera
//! that saw it, from its four corner pixels. Shared by phones and robots.
use anyhow::{Result, ensure};
use auki_geometry::pnp::{Camera, Vector2, Vector3, estimate_square_pose_from_pixels, pose_tools};

/// A printed or shown square of known side in the camera that saw it: the
/// marker frame (x right, y up as the code reads, z out of its face) in the
/// OpenGL camera frame. Rejects bad corner order, tiny, distant, back-facing
/// and badly reprojecting squares. `k` is fx, fy, cx, cy; `size` the image.
pub fn square_in_camera(
    k: [f64; 4],
    size: [usize; 2],
    corners: [[f64; 2]; 4],
    side_length_m: f64,
) -> Result<auki_odometry::Pose> {
    let mut area = 0.;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let d = corners[(i + 2) % 4];
        ensure!(
            a.iter().all(|v| v.is_finite())
                && a[0] >= 0.
                && a[1] >= 0.
                && a[0] < size[0] as f64
                && a[1] < size[1] as f64
                && (b[0] - a[0]) * (d[1] - b[1]) - (b[1] - a[1]) * (d[0] - b[0]) > 1e-9,
            "Invalid printed corner order"
        );
        area += a[0] * b[1] - a[1] * b[0];
    }
    ensure!(area / 2. >= 64., "Portal too small");
    let camera =
        Camera::pinhole(k[0], k[1], k[2], k[3]).map_err(|_| anyhow::anyhow!("Invalid camera"))?;
    // SDK returns marker -> OpenGL camera, the same axes used by ARCore Pose.
    let estimate = estimate_square_pose_from_pixels(
        corners.map(|p| Vector2::new(p[0], p[1])),
        side_length_m,
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
    let h = side_length_m / 2.;
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
    Ok(auki_odometry::Pose {
        translation: [p.position.x, p.position.y, p.position.z],
        rotation_xyzw: [p.rotation.x, p.rotation.y, p.rotation.z, p.rotation.w],
    })
}

