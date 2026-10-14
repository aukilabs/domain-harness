//! Single-client C ABI. Swift owns returned strings and releases them with
//! spatial_mobile_string_free. All peer work and cleanup stay on Tokio.
use spatial_home::mobile::{MobileClient, SignIn};

/// Submit a bounded phone-local map, or null to stop sharing it.
/// # Safety
/// Non-null input must be a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_local_map(value: *const std::ffi::c_char) -> i32 {
    std::panic::catch_unwind(|| {
        let sample = if value.is_null() { None } else {
            let bytes = unsafe { std::ffi::CStr::from_ptr(value) }.to_bytes();
            if bytes.len() > 512 * 1024 { return 0; }
            let Ok(s) = serde_json::from_slice::<spatial_home::mobile_local::Snapshot>(bytes) else {return 0};
            if s.validate().is_err() { return 0; } Some(s)
        };
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if client.finished() { return 0; }
            client.update_local_map(sample); return 1;
        }
        0
    }).unwrap_or(0)
}
use std::{
    ffi::{CStr, CString, c_char},
    sync::{Arc, Mutex, OnceLock},
};

static CLIENT: Mutex<Option<Arc<MobileClient>>> = Mutex::new(None);

/// # Safety
/// `request` must be a NUL-terminated UTF-8 JSON string valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_change_note(request: *const c_char) -> i32 {
    std::panic::catch_unwind(|| {
        if request.is_null() { return 0; }
        let bytes = unsafe { CStr::from_ptr(request) }.to_bytes();
        if bytes.len() > 2*1024*1024 { return 0; }
        let Ok(request) = serde_json::from_slice(bytes) else { return 0; };
        CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .map_or(0, |client| i32::from(client.change_note(request)))
    }).unwrap_or(0)
}

/// Report a Portal the scene does not know; Home decides whether to add it.
/// # Safety
/// `report` must be a NUL-terminated UTF-8 JSON string valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_report_portal(report: *const c_char) -> i32 {
    std::panic::catch_unwind(|| {
        if report.is_null() { return 0; }
        let bytes = unsafe { CStr::from_ptr(report) }.to_bytes();
        if bytes.len() > 8192 { return 0; }
        let Ok(report) = serde_json::from_slice(bytes) else { return 0; };
        CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .map_or(0, |client| i32::from(client.report_portal(report)))
    }).unwrap_or(0)
}

/// Add a drawn path to Home's scene, or remove a saved one. The outcome arrives
/// in the polled `local_detail` as "Path <id> …".
/// # Safety
/// `change` must be a NUL-terminated UTF-8 JSON string valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_change_path(change: *const c_char) -> i32 {
    std::panic::catch_unwind(|| {
        if change.is_null() { return 0; }
        let bytes = unsafe { CStr::from_ptr(change) }.to_bytes();
        if bytes.len() > 2 * 1024 * 1024 { return 0; }
        let Ok(change) = serde_json::from_slice(bytes) else { return 0; };
        CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .map_or(0, |client| i32::from(client.change_path(change)))
    }).unwrap_or(0)
}

/// Ask Home to plan a route, or to take this phone's route away: `{"id", "ask"}`.
/// The answer arrives in the polled `route` under the same `id`.
/// # Safety
/// `asked` must be a NUL-terminated UTF-8 JSON string valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_ask_route(asked: *const c_char) -> i32 {
    std::panic::catch_unwind(|| {
        if asked.is_null() { return 0; }
        let bytes = unsafe { CStr::from_ptr(asked) }.to_bytes();
        if bytes.len() > 8192 { return 0; }
        let Ok(asked) = serde_json::from_slice(bytes) else { return 0; };
        CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .map_or(0, |client| i32::from(client.ask_route(asked)))
    }).unwrap_or(0)
}

/// Advertise camera availability while scanning and query SDK subscription demand.
/// Queue an unlocalized first-Portal measurement against the exact empty scene
/// the UI captured. The bridge supplies the full authenticated SDK snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_report_origin(raw:*const c_char)->i32 {
    std::panic::catch_unwind(||{
        if raw.is_null(){return 0;}
        let bytes=unsafe{CStr::from_ptr(raw)}.to_bytes();if bytes.len()>8192{return 0;}
        let Ok(m)=serde_json::from_slice::<spatial_home::scene_origin::PhoneMeasurement>(bytes) else{return 0};
        let client=CLIENT.lock().unwrap_or_else(|e|e.into_inner());let Some(client)=client.as_ref() else{return 0};
        let Ok(reference)=client.origin_reference() else{return 0};
        if reference.scenegraph.map.frame.id!=m.expected_frame || reference.scenegraph.map.frame.origin_description!=m.expected_origin_description{return 0;}
        i32::from(client.report_origin(spatial_home::scene_origin::Observation{expected_scene:reference,code:m.code,side_length_m:m.side_length_m,timestamp_ns:m.timestamp_ns,portal_to_native:m.portal_to_native}))
    }).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn spatial_mobile_camera_demand(active: i32) -> i32 {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        .map_or(0, |client| i32::from(client.camera_demanded(active != 0)))
}

/// # Safety
/// `jpeg` must point to `length` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_camera(timestamp: u64, jpeg: *const u8, length: usize) {
    if jpeg.is_null() || length == 0 || length > spatial_home::mobile_camera::MAX_JPEG || timestamp == 0 { return; }
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        client.update_camera(timestamp, unsafe { std::slice::from_raw_parts(jpeg, length) }.to_vec());
    }
}
/// A camera frame with the AR session that took it and the pinhole intrinsics
/// of the image as sent (pixels, origin top-left). `rotation_deg` is how far
/// the image was already turned clockwise from the camera's own orientation:
/// a 4:3 image 640 to 1280 on its long side, on its side for 90 or 270.
/// `alignment_timestamp_ns` is the capture time of the Portal alignment in
/// use, or 0 when the phone is not aligned.
/// # Safety
/// `jpeg` must point to `length` readable bytes and `session` must be
/// NUL-terminated UTF-8, both for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_camera_calibrated(
    timestamp: u64, jpeg: *const u8, length: usize, session: *const c_char,
    fx: f64, fy: f64, cx: f64, cy: f64, rotation_deg: i32, alignment_timestamp_ns: u64,
) {
    if jpeg.is_null() || session.is_null() || length == 0 || length > spatial_home::mobile_camera::MAX_JPEG || timestamp == 0 { return; }
    let Ok(session) = unsafe { CStr::from_ptr(session) }.to_str() else { return };
    if session.len() > 64 { return; }
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        client.update_camera_calibrated(
            timestamp,
            unsafe { std::slice::from_raw_parts(jpeg, length) }.to_vec(),
            spatial_home::mobile_camera::Geometry { session: session.into(), fx, fy, cx, cy, rotation_deg: rotation_deg.rem_euclid(360) as u16, alignment_timestamp_ns },
        );
    }
}
/// The LiDAR depth image of the camera frame with the same `timestamp`: not
/// turned, row-major from the top-left pixel, depths in millimetres (0 = none)
/// and confidence 0..2, with the pinhole intrinsics of the depth image.
/// # Safety
/// `depth_mm` and `confidence` must each point to `width * height` readable
/// elements and `session` must be NUL-terminated UTF-8, for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_depth_image(
    timestamp: u64, session: *const c_char, width: u32, height: u32,
    depth_mm: *const u16, confidence: *const u8,
    fx: f64, fy: f64, cx: f64, cy: f64, alignment_timestamp_ns: u64,
) {
    let side = 1..=spatial_home::mobile_depth_image::MAX_SIDE;
    if depth_mm.is_null() || confidence.is_null() || session.is_null() || timestamp == 0
        || !side.contains(&width) || !side.contains(&height) { return; }
    let Ok(session) = unsafe { CStr::from_ptr(session) }.to_str() else { return };
    let pixels = width as usize * height as usize;
    let (depth_mm, confidence) = unsafe {
        (std::slice::from_raw_parts(depth_mm, pixels), std::slice::from_raw_parts(confidence, pixels))
    };
    let Ok(image) = spatial_home::mobile_depth_image::DepthImage::new(
        session, timestamp, (width, height), depth_mm, confidence, [fx, fy, cx, cy], alignment_timestamp_ns,
    ) else { return };
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        client.update_depth_image(image);
    }
}
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Pure local detection. Caller frees the result with spatial_mobile_string_free.
/// # Safety
/// luma must span length readable bytes; capture must be NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_calibrate(luma: *const u8, length: usize, capture: *const c_char) -> *mut c_char {
    std::panic::catch_unwind(|| {
        if luma.is_null() || capture.is_null() || length == 0 || length > 1920*1920 { return std::ptr::null_mut(); }
        let metadata = unsafe { CStr::from_ptr(capture) }.to_bytes();
        if metadata.len() > 1024*1024 { return std::ptr::null_mut(); }
        let parsed = serde_json::from_slice::<spatial_home::mobile_calibration::Capture>(metadata);
        let report = match parsed {
            Ok(c) => spatial_home::mobile_calibration::detect(unsafe { std::slice::from_raw_parts(luma, length) }, &c)
                .map(|r| {if let Some(observation)=&r.origin_observation {if let Some(client)=CLIENT.lock().unwrap_or_else(|e|e.into_inner()).as_ref(){client.report_origin(observation.clone());}} serde_json::to_value(r).unwrap()}).unwrap_or_else(|e| serde_json::json!({"detail":format!("Calibration unavailable: {e}"),"alignment":null})),
            Err(_) => serde_json::json!({"detail":"Invalid calibration capture","alignment":null}),
        };
        CString::new(report.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
    }).unwrap_or(std::ptr::null_mut())
}

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("mobile Tokio runtime")
    })
}

/// Sign in with an Auki User email and password.
/// # Safety
/// Each argument must be a valid NUL-terminated UTF-8 string for the duration of
/// this call. Inputs are copied before returning; no Swift pointers are retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_start(
    profile: *const c_char,
    email: *const c_char,
    password: *const c_char,
    identity: *const c_char,
) -> i32 {
    unsafe { start(profile, email, password, identity, |email, password| SignIn::User { email, password }) }
}

/// Sign in with an app key and secret, as Home does.
/// # Safety
/// Same contract as `spatial_mobile_start`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_start_app(
    profile: *const c_char,
    key: *const c_char,
    secret: *const c_char,
    identity: *const c_char,
) -> i32 {
    unsafe { start(profile, key, secret, identity, |key, secret| SignIn::App { key, secret }) }
}

/// # Safety
/// Each pointer must be a valid NUL-terminated UTF-8 string for the duration of this call.
unsafe fn start(
    profile: *const c_char,
    first: *const c_char,
    second: *const c_char,
    identity: *const c_char,
    sign_in: impl FnOnce(String, String) -> SignIn,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let read = |pointer: *const c_char| {
            if pointer.is_null() {
                return None;
            }
            // SAFETY: the caller supplies live, NUL-terminated strings.
            unsafe { CStr::from_ptr(pointer) }
                .to_str()
                .ok()
                .map(str::to_owned)
        };
        let (Some(profile), Some(first), Some(second), Some(identity)) =
            (read(profile), read(first), read(second), read(identity))
        else {
            return 0;
        };
        let mut slot = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|client| !client.finished()) {
            return 0;
        }
        let client = Arc::new(MobileClient::default());
        runtime().spawn(client.clone().run(profile, sign_in(first, second), identity));
        *slot = Some(client);
        1
    }))
    .unwrap_or(0)
}

/// Returns a JSON snapshot or null. No credentials or private identity data.
#[unsafe(no_mangle)]
pub extern "C" fn spatial_mobile_poll() -> *mut c_char {
    std::panic::catch_unwind(|| {
        let slot = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(client) = slot.as_ref() else {
            return std::ptr::null_mut();
        };
        // Read completion before the state: a completed response must include
        // the final status published before the Release store in MobileClient.
        let finished = client.finished();
        let state = serde_json::json!({ "update": client.snapshot(), "finished": finished });
        CString::new(state.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Nonblocking. Poll until finished=true before starting another connection.
#[unsafe(no_mangle)]
pub extern "C" fn spatial_mobile_stop() {
    let _ = std::panic::catch_unwind(|| {
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.stop();
        }
    });
}

/// # Safety
/// `value` must be null or an unreleased pointer returned by spatial_mobile_poll.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_string_free(value: *mut c_char) {
    if !value.is_null() {
        // SAFETY: only pointers allocated by CString::into_raw are accepted.
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_rejects_null_inputs() {
        assert_eq!(
            unsafe {
                spatial_mobile_start(
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let null = std::ptr::null();
        assert_eq!(unsafe { spatial_mobile_start_app(null, null, null, null) }, 0);
    }
    #[test]
    fn invalid_profile_finishes_without_network_or_credentials_in_output() {
        // One shared client: the User and app-key sign-ins run one after the other.
        type Start = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char) -> i32;
        let starts: [(Start, &str); 2] = [
            (spatial_mobile_start, "test@example.invalid"),
            (spatial_mobile_start_app, "test-app-key"),
        ];
        for (start, first) in starts {
            let profile = CString::new("{}").unwrap();
            let first = CString::new(first).unwrap();
            let secret = CString::new("never-log-this-test-secret").unwrap();
            let path = CString::new("/unused").unwrap();
            assert_eq!(
                unsafe { start(profile.as_ptr(), first.as_ptr(), secret.as_ptr(), path.as_ptr()) },
                1
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let raw = spatial_mobile_poll();
                assert!(!raw.is_null());
                let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_owned();
                unsafe { spatial_mobile_string_free(raw) };
                assert!(!text.contains("never-log"));
                let state: serde_json::Value = serde_json::from_str(&text).unwrap();
                if state["finished"] == true {
                    assert_eq!(state["update"]["state"], "error");
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

/// Queue a bounded annotation request on the existing authenticated Home peer.
/// # Safety
/// `request` must point to a NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_annotate(request: *const c_char) -> i32 {
    std::panic::catch_unwind(|| {
        if request.is_null() {
            return 0;
        }
        let bytes = unsafe { CStr::from_ptr(request) }.to_bytes();
        if bytes.len() > 4096 {
            return 0;
        }
        let Ok(request) = serde_json::from_slice(bytes) else {
            return 0;
        };
        let slot = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        i32::from(slot.as_ref().is_some_and(|client| client.annotate(request)))
    })
    .unwrap_or(0)
}

/// Update the latest pose; null withdraws phone presence.
/// # Safety
/// A non-null pointer must be a NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_pose(value: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        let sample = if value.is_null() {
            None
        } else {
            let bytes = unsafe { CStr::from_ptr(value) }.to_bytes();
            if bytes.len() > 4096 {
                return;
            }
            let Ok(sample) = serde_json::from_slice(bytes) else {
                return;
            };
            Some(sample)
        };
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.update_pose(sample);
        }
    });
}
/// # Safety
/// Configuration must be a NUL-terminated UTF-8 JSON string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_configure_camera(value:*const c_char){
 let _=std::panic::catch_unwind(||{
  if value.is_null(){return;}let bytes=unsafe{CStr::from_ptr(value)}.to_bytes();if bytes.len()>4096{return;}
  if let Ok(config)=serde_json::from_slice(bytes){if let Some(client)=CLIENT.lock().unwrap_or_else(|e|e.into_inner()).as_ref(){let _=client.configure_camera(config);}}
 });
}

/// Set the name Home shows for this phone. Null or empty leaves it unchanged.
/// # Safety
/// A non-null `name` must be a NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_name(name: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        if name.is_null() { return; }
        let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
        let Ok(name) = std::str::from_utf8(bytes) else { return };
        if name.len() > 256 { return; }
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.set_name(name.to_owned());
        }
    });
}

/// Tell Home where the scene is on the Earth: a `spatial.phone-geo/v1` JSON reference.
/// # Safety
/// A non-null `reference` must be a NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_geo(reference: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        if reference.is_null() { return; }
        let bytes = unsafe { CStr::from_ptr(reference) }.to_bytes();
        if bytes.len() > 4096 { return; }
        let Ok(reference) = serde_json::from_slice::<spatial_home::mobile_geo::Reference>(bytes) else { return };
        if reference.validate(&reference.frame).is_err() { return; }
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.set_geo(reference);
        }
    });
}

/// Hold the joystick for `target` (a robot peer ID) at `forward`/`turn` in
/// -1...1. A null target releases it, which sends stops.
/// # Safety
/// A non-null `target` must be a NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_drive(target: *const c_char, forward: f32, turn: f32) {
    let _ = std::panic::catch_unwind(|| {
        let input = if target.is_null() || !forward.is_finite() || !turn.is_finite() {
            None
        } else {
            let bytes = unsafe { CStr::from_ptr(target) }.to_bytes();
            match std::str::from_utf8(bytes) {
                Ok(id) if !id.is_empty() && id.len() <= 128 => Some((id.to_owned(), forward, turn)),
                _ => None,
            }
        };
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.set_drive(input);
        }
    });
}

/// Report that the phone paused (1) or resumed (0) its camera.
#[unsafe(no_mangle)]
pub extern "C" fn spatial_mobile_sleep(asleep: i32) {
    if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        client.set_asleep(asleep != 0);
    }
}

/// Where the AR session sits in the scene: `{"session", "timestamp_ns", "session_to_scene"}`.
/// Null says the phone no longer knows. Published apart from depth sharing.
/// # Safety
/// A non-null pointer must be NUL-terminated UTF-8 valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_placement(value: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        let placement = if value.is_null() {
            None
        } else {
            let bytes = unsafe { CStr::from_ptr(value) }.to_bytes();
            if bytes.len() > 8192 { return; }
            let Ok(placement) = serde_json::from_slice(bytes) else { return; };
            Some(placement)
        };
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.update_placement(placement);
        }
    });
}

/// Replace the latest raw phone-local depth capture; null pauses sharing.
/// # Safety
/// A non-null pointer must be NUL-terminated UTF-8 valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_mobile_depth(value: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        let sample = if value.is_null() {
            None
        } else {
            let bytes = unsafe { CStr::from_ptr(value) }.to_bytes();
            if bytes.len() > 512 * 1024 {
                return;
            }
            let Ok(sample) = serde_json::from_slice::<spatial_home::mobile_depth::Sample>(bytes)
            else {
                return;
            };
            if sample.points.len() > 2048 {
                return;
            }
            Some(sample)
        };
        if let Some(client) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            client.update_depth(sample);
        }
    });
}
