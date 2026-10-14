/// Retry scale for frames whose Portals do not decode at native resolution.
/// A 15 cm Portal a few metres from a 1280-px head eye spans under 2 px per
/// module and the scanner's own small-code rungs do not fire in cluttered
/// scenes. On live BB Lab frames 2× decoded 14 of 24 eye images where 1.5×
/// managed 5 and native none.
pub const UPSCALE_RETRY: Upscale = Upscale::X2;

/// Catmull-Rom upscales with fixed integer taps. Output pixel o samples
/// source position (o+0.5)/f-0.5, so the tap pattern repeats every period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Upscale {
    X1_5,
    X2,
}
impl Upscale {
    pub fn factor(self) -> f64 {
        match self {
            Upscale::X1_5 => 1.5,
            Upscale::X2 => 2.,
        }
    }
    /// (outputs per period, source pixels per period, per-phase (first tap
    /// offset, weights), weight sum).
    fn kernel(self) -> (usize, usize, &'static [(isize, [i32; 4])], i32) {
        match self {
            // Phases sample t=2/3 from 2k-1, t=1/3 from 2k, and 2k+1 exactly.
            Upscale::X1_5 => (
                3,
                2,
                &[
                    (-2, [-1, 9, 21, -2]),
                    (-1, [-2, 21, 9, -1]),
                    (1, [27, 0, 0, 0]),
                ],
                27,
            ),
            // Phases sample t=3/4 from k-1 and t=1/4 from k.
            Upscale::X2 => (
                2,
                1,
                &[(-2, [-3, 29, 111, -9]), (-1, [-9, 111, 29, -3])],
                128,
            ),
        }
    }
}

/// Separable integer upscale; ~5× faster than the generic resizer, which also
/// clips the intermediate pass.
pub fn upscale(gray: &image::GrayImage, scale: Upscale) -> image::GrayImage {
    let (period, step, phases, sum) = scale.kernel();
    let taps = |o: usize, len: usize| -> [(usize, i32); 4] {
        let (offset, weights) = phases[o % period];
        let first = ((o / period) * step) as isize + offset;
        std::array::from_fn(|i| {
            (
                (first + i as isize).clamp(0, len as isize - 1) as usize,
                weights[i],
            )
        })
    };
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let (ow, oh) = (w * period / step, h * period / step);
    let src = gray.as_raw();
    let columns: Vec<_> = (0..ow).map(|o| taps(o, w)).collect();
    let mut rows = vec![0i32; ow * h]; // horizontal pass, scaled by `sum`
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for (out, t) in rows[y * ow..(y + 1) * ow].iter_mut().zip(&columns) {
            *out = t.iter().map(|&(x, wt)| wt * i32::from(row[x])).sum();
        }
    }
    let (norm, half) = (sum * sum, sum * sum / 2);
    let mut out = vec![0u8; ow * oh];
    for o in 0..oh {
        let t = taps(o, h);
        for (x, px) in out[o * ow..(o + 1) * ow].iter_mut().enumerate() {
            let v: i32 = t.iter().map(|&(y, wt)| wt * rows[y * ow + x]).sum();
            *px = ((v + half).div_euclid(norm)).clamp(0, 255) as u8;
        }
    }
    image::GrayImage::from_raw(ow as u32, oh as u32, out).expect("sized buffer")
}

/// One decoded QR code with corners in the scanned image's own pixels.
#[derive(Clone, Debug)]
pub struct Code {
    pub payload: String,
    pub mirrored: bool,
    /// Top-left, top-right, bottom-right, bottom-left (refined when available).
    pub corners_px: [[f64; 2]; 4],
}
impl Code {
    pub fn portal(&self) -> Option<&str> {
        (!self.mirrored)
            .then(|| portal_code(&self.payload))
            .flatten()
    }
}

/// Every QR code in `gray`. When `retry` is set and no Portal decodes at
/// native resolution, scans once more on an [`UPSCALE_RETRY`] copy and keeps
/// that result if it holds a Portal. Returns (codes, decoded on the retry).
pub fn detect_codes(
    detector: &mut auki_qr_detector::QrDetector,
    gray: &image::GrayImage,
    retry: bool,
) -> anyhow::Result<(Vec<Code>, bool)> {
    let native = scan(detector, gray, 1.)?;
    if !retry || native.iter().any(|c| c.portal().is_some()) {
        return Ok((native, false));
    }
    let larger = upscale(gray, UPSCALE_RETRY);
    let retried = scan(detector, &larger, UPSCALE_RETRY.factor())?;
    Ok(if retried.iter().any(|c| c.portal().is_some()) {
        (retried, true)
    } else {
        (native, false)
    })
}

fn scan(
    detector: &mut auki_qr_detector::QrDetector,
    gray: &image::GrayImage,
    scale: f64,
) -> anyhow::Result<Vec<Code>> {
    let found = detector.detect_luma(
        gray.as_raw(),
        gray.width() as usize,
        gray.height() as usize,
        gray.width() as usize,
    )?;
    // Pixel centres: x_source = (x_scaled + 0.5) / scale - 0.5.
    let source = |v: f64| (v + 0.5) / scale - 0.5;
    Ok(found
        .codes
        .iter()
        .map(|c| Code {
            payload: c.payload.clone(),
            mirrored: c.mirrored,
            corners_px: c
                .refined_corners_px
                .as_ref()
                .unwrap_or(&c.corners_px)
                .map(|p| [source(p.x), source(p.y)]),
        })
        .collect())
}

pub fn portal_code(payload: &str) -> Option<&str> {
    let (prefix, code) = payload.rsplit_once('/')?;
    (prefix.eq_ignore_ascii_case("https://r8.hr")
        && code.len() == 11
        && code
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
    .then_some(code)
}
