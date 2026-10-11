//! Read-only offline diagnostic; never changes the live camera or detector.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("JPEG path");
    let gray = image::open(path)?.to_luma8();
    for (name, pixels) in [
        ("full", gray.clone()),
        ("lower-center", image::imageops::crop_imm(&gray, 550, 650, 220, 230).to_image()),
    ] {
        for scale in [1, 2, 3] {
            if name == "full" && scale > 1 { continue; }
            let pixels = image::imageops::resize(&pixels, pixels.width()*scale, pixels.height()*scale, image::imageops::FilterType::CatmullRom);
            let mut detector = auki_qr_detector::QrDetector::default();
            let start = std::time::Instant::now();
            let result = detector.detect_luma(pixels.as_raw(), pixels.width() as usize, pixels.height() as usize, pixels.width() as usize)?;
            println!("{name} scale={scale} ms={} codes={:?}", start.elapsed().as_millis(), result.codes.iter().map(|c| &c.payload).collect::<Vec<_>>());
        }
    }
    Ok(())
}
