use anyhow::Result;
use base64::Engine;
fn main() -> Result<()> {
    let file = std::env::args().nth(1).expect("preview JSON");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file)?)?;
    if let Some(frames) = v.as_array() {
        for temporal in [false, true] {
            let mut config = auki_qr_detector::QrDetectorConfig::default();
            if temporal {
                config.scanner = config.scanner.temporal(Default::default());
            }
            let mut detectors = std::collections::BTreeMap::new();
            for frame in frames {
                let name = frame["camera"].as_str().unwrap();
                let detector = detectors
                    .entry(name)
                    .or_insert_with(|| auki_qr_detector::QrDetector::new(config));
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(frame["data"].as_str().unwrap())?;
                let gray = image::load_from_memory(&bytes)?.to_luma8();
                let result = detector.detect_luma(
                    gray.as_raw(),
                    gray.width() as usize,
                    gray.height() as usize,
                    gray.width() as usize,
                )?;
                println!(
                    "temporal={temporal} camera={name} frame={} codes={:?}",
                    frame["index"],
                    result.codes.iter().map(|c| &c.payload).collect::<Vec<_>>()
                );
            }
        }
        return Ok(());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap())?;
    let rgb = image::load_from_memory(&bytes)?.to_rgb8();
    let gray = image::DynamicImage::ImageRgb8(rgb.clone()).to_luma8();
    let (w, h) = gray.dimensions();
    if std::env::var_os("QR_THRESHOLDS").is_some() {
        for threshold in (40..240).step_by(8) {
            let binary = image::GrayImage::from_fn(w, h, |x, y| {
                image::Luma([if gray.get_pixel(x, y)[0] < threshold {
                    0
                } else {
                    255
                }])
            });
            let mut detector = auki_qr_detector::QrDetector::default();
            let result =
                detector.detect_luma(binary.as_raw(), w as usize, h as usize, w as usize)?;
            println!(
                "threshold={threshold} codes={:?}",
                result.codes.iter().map(|c| &c.payload).collect::<Vec<_>>()
            );
        }
        return Ok(());
    }
    for channel in 0..4 {
        let plane = if channel == 0 {
            gray.clone()
        } else {
            image::GrayImage::from_fn(w, h, |x, y| image::Luma([rgb.get_pixel(x, y)[channel - 1]]))
        };
        for size in [0, 256, 384, 512] {
            let size = if size == 0 { w.max(h) } else { size };
            let step = (size / 2).max(1);
            let mut config = auki_qr_detector::QrDetectorConfig::default();
            config.scanner.scan.enable_shadow_normalization = true;
            config.scanner.scan.enable_sharpening = true;
            config.scanner.scan.enable_deblur = true;
            config.scanner.scan.enable_early_exit = false;
            config.scanner.scan.max_variants_per_frame = 64;
            let mut detector = auki_qr_detector::QrDetector::new(config);
            for y in (0..h).step_by(step as usize) {
                for x in (0..w).step_by(step as usize) {
                    let cw = size.min(w - x);
                    let ch = size.min(h - y);
                    if cw < 128 || ch < 128 {
                        continue;
                    }
                    let crop = image::imageops::crop_imm(&plane, x, y, cw, ch).to_image();
                    for scale in [1, 2, 3] {
                        if size >= w.max(h) && scale > 1 {
                            continue;
                        }
                        let resized = image::imageops::resize(
                            &crop,
                            cw * scale,
                            ch * scale,
                            image::imageops::FilterType::Triangle,
                        );
                        let result = detector.detect_luma(
                            resized.as_raw(),
                            (cw * scale) as usize,
                            (ch * scale) as usize,
                            (cw * scale) as usize,
                        )?;
                        for code in result.codes {
                            println!(
                                "channel={channel} size={size} scale={scale} roi={x},{y},{cw},{ch} payload={} mirrored={}",
                                code.payload, code.mirrored
                            );
                        }
                    }
                    if size >= w.max(h) {
                        break;
                    }
                }
                if size >= w.max(h) {
                    break;
                }
            }
        }
    }
    Ok(())
}
