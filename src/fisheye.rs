//! Kannala-Brandt fisheye corner rectification, matching OpenCV's
//! `cv2.fisheye.undistortPoints(points, K, D, R=R, P=P)`.
//!
//! Detecting on a whole rectified image resamples every pixel and, on the
//! Bracketbot head camera, shrinks the image centre to 0.72×. Detecting on the
//! unrectified eye and rectifying only the corners keeps small Portals legible.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// One unrectified eye and the calibration that maps it into the rectified
/// camera the pose solver uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnrectifiedEye {
    pub width: u32,
    pub height: u32,
    /// Grey eye image, JPEG-encoded to keep the sample small.
    pub jpeg_base64: String,
    /// Fisheye intrinsics [fx, fy, cx, cy] in this image's pixels (no skew).
    pub k: [f64; 4],
    /// Kannala-Brandt coefficients k1..k4.
    pub d: [f64; 4],
    /// Row-major rectification rotation (OpenCV R1/R2).
    pub r: [f64; 9],
}

impl UnrectifiedEye {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (64..=4096).contains(&self.width) && (64..=4096).contains(&self.height),
            "unexpected unrectified eye shape"
        );
        ensure!(
            self.k.iter().chain(&self.d).chain(&self.r).all(|v| v.is_finite())
                && self.k[0] > 0.
                && self.k[1] > 0.,
            "invalid fisheye calibration"
        );
        let r = &self.r;
        let rows = [[r[0], r[1], r[2]], [r[3], r[4], r[5]], [r[6], r[7], r[8]]];
        for (i, a) in rows.iter().enumerate() {
            for (j, b) in rows.iter().enumerate() {
                let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
                ensure!(
                    (dot - f64::from(u8::from(i == j))).abs() < 1e-6,
                    "rectification R is not a rotation"
                );
            }
        }
        Ok(())
    }

    /// Unrectified pixel → rectified pixel for intrinsics `p` = [fx, fy, cx, cy].
    /// `None` when the ray falls behind the rectified camera or the distortion
    /// model cannot be inverted.
    pub fn rectify(&self, pixel: [f64; 2], p: [f64; 4]) -> Option<[f64; 2]> {
        let [fx, fy, cx, cy] = self.k;
        let [k1, k2, k3, k4] = self.d;
        let (x, y) = ((pixel[0] - cx) / fx, (pixel[1] - cy) / fy);
        let theta_d = (x * x + y * y).sqrt().min(std::f64::consts::FRAC_PI_2);
        let scale = if theta_d > 1e-8 {
            let mut theta = theta_d;
            for _ in 0..10 {
                let t2 = theta * theta;
                let (t4, t6, t8) = (t2 * t2, t2 * t2 * t2, t2 * t2 * t2 * t2);
                let f = theta * (1. + k1 * t2 + k2 * t4 + k3 * t6 + k4 * t8) - theta_d;
                let df = 1. + 3. * k1 * t2 + 5. * k2 * t4 + 7. * k3 * t6 + 9. * k4 * t8;
                let step = f / df;
                theta -= step;
                if step.abs() < 1e-10 {
                    break;
                }
            }
            if theta < 0. || theta >= std::f64::consts::FRAC_PI_2 {
                return None;
            }
            theta.tan() / theta_d
        } else {
            1.
        };
        let (u, v) = (x * scale, y * scale);
        let r = &self.r;
        let ray = [
            r[0] * u + r[1] * v + r[2],
            r[3] * u + r[4] * v + r[5],
            r[6] * u + r[7] * v + r[8],
        ];
        if ray[2] <= 1e-9 {
            return None;
        }
        let out = [p[0] * ray[0] / ray[2] + p[2], p[1] * ray[1] / ray[2] + p[3]];
        out.iter().all(|v| v.is_finite()).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0125's head-camera calibration with OpenCV's own undistortPoints output.
    #[test]
    fn matches_opencv_fisheye_undistort_points_on_both_eyes() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/bracketbot-0125-fisheye.json")).unwrap();
        for side in ["left", "right"] {
            let e = &fixture[side];
            let m = |key: &str| -> Vec<f64> {
                e[key].as_array().unwrap().iter().flat_map(|row| {
                    row.as_array().map_or_else(|| vec![row.as_f64().unwrap()], |r| r.iter().map(|v| v.as_f64().unwrap()).collect())
                }).collect()
            };
            let (k, d, r, p) = (m("K"), m("D"), m("R"), m("P"));
            let eye = UnrectifiedEye {
                width: 1280,
                height: 960,
                jpeg_base64: String::new(),
                k: [k[0], k[4], k[2], k[5]],
                d: d.try_into().unwrap(),
                r: r.try_into().unwrap(),
            };
            eye.validate().unwrap();
            // P is 3×4; undistortPoints uses its 3×3 part, so the right eye's
            // baseline term does not move rectified pixels.
            let target = [p[0], p[5], p[2], p[6]];
            for (point, expected) in e["points"].as_array().unwrap().iter().zip(e["expected"].as_array().unwrap()) {
                let point = [point[0].as_f64().unwrap(), point[1].as_f64().unwrap()];
                let expected = [expected[0].as_f64().unwrap(), expected[1].as_f64().unwrap()];
                let got = eye.rectify(point, target).expect("in front of the camera");
                assert!(
                    (got[0] - expected[0]).abs() < 1e-3 && (got[1] - expected[1]).abs() < 1e-3,
                    "{side} {point:?}: got {got:?}, OpenCV {expected:?}"
                );
            }
        }
    }

    #[test]
    fn rejects_non_rotations_and_bad_intrinsics() {
        let mut eye = UnrectifiedEye {
            width: 1280,
            height: 960,
            jpeg_base64: String::new(),
            k: [447., 447., 640., 480.],
            d: [0.1, -0.03, 0., 0.],
            r: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        };
        eye.validate().unwrap();
        eye.r[0] = 2.;
        assert!(eye.validate().is_err());
        eye.r[0] = 1.;
        eye.k[0] = 0.;
        assert!(eye.validate().is_err());
    }
}
