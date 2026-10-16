//! The robot reads the host phone's 50 mm onboarding marker through the same
//! QR path it reads Portals with (`qr_decode::detect_codes`), at the sizes a
//! head camera sees it: a marker too small to decode natively must still come
//! through the 2x retry, not be dropped for not being a Portal.

use image::{GrayImage, Luma};
use spatial_home::{mobile_onboarding::qr_modules, onboarding, qr_decode};

/// A grey 640x480 camera frame with `text` drawn `side` pixels wide (quiet
/// zone excluded, as the phone draws it on a white card), JPEG round-tripped.
fn frame(text: &str, side: f64) -> GrayImage {
    let (width, modules) = qr_modules(text).unwrap();
    let module = side / width as f64;
    let (x0, y0) = (300., 200.);
    let quiet = 4. * module;
    let mut img = GrayImage::from_fn(640, 480, |x, y| Luma([90 + ((x * 7 + y * 13) % 23) as u8]));
    for y in 0..480u32 {
        for x in 0..640u32 {
            let (fx, fy) = (x as f64 + 0.5 - x0, y as f64 + 0.5 - y0);
            if fx < -quiet || fy < -quiet || fx >= side + quiet || fy >= side + quiet { continue; }
            let (c, r) = ((fx / module).floor(), (fy / module).floor());
            let dark = c >= 0. && r >= 0. && (c as usize) < width && (r as usize) < width
                && modules[r as usize * width + c as usize];
            img.put_pixel(x, y, Luma([if dark { 25 } else { 225 }]));
        }
    }
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).unwrap();
    image::load_from_memory(&jpeg).unwrap().to_luma8()
}

fn marker() -> String {
    onboarding::marker_payload("0123456789abcdef0123456789abcdef")
}

#[test]
fn marker_is_a_version_1_code() {
    let width = qr_modules(&marker()).unwrap().0;
    assert_eq!(width, 21, "marker {} is {width} modules wide", marker());
}

#[test]
fn reads_the_marker_at_head_camera_sizes() {
    let mut detector = auki_qr_detector::QrDetector::new(Default::default());
    for side in [40., 60., 90., 140.] {
        let (codes, _) = qr_decode::detect_codes(&mut detector, &frame(&marker(), side), true).unwrap();
        let code = codes.iter().find(|c| c.payload == marker())
            .unwrap_or_else(|| panic!("marker {side} px wide not read"));
        assert!((code.corners_px[0][0] - 300.).abs() < 3. && (code.corners_px[0][1] - 200.).abs() < 3.,
            "{side} px: corner {:?}", code.corners_px[0]);
    }
}

#[test]
fn keeps_a_marker_only_the_retry_reads() {
    let mut detector = auki_qr_detector::QrDetector::new(Default::default());
    let small = (8..40).map(|s| s as f64).find(|&side| {
        let gray = frame(&marker(), side);
        let native = qr_decode::detect_codes(&mut detector, &gray, false).unwrap().0;
        !native.iter().any(|c| c.payload == marker())
            && qr_decode::upscale(&gray, qr_decode::UPSCALE_RETRY).width() > 0
            && {
                let (codes, retried) = qr_decode::detect_codes(&mut detector, &gray, true).unwrap();
                retried && codes.iter().any(|c| c.payload == marker())
            }
    });
    assert!(small.is_some(), "no size where only the 2x retry reads the marker");
}
