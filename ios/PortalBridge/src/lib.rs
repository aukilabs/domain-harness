//! Single-client C ABI. Swift owns returned strings and releases them with
//! spatial_mobile_string_free. All peer work and cleanup stay on Tokio.
use spatial_home::mobile::MobileClient;
use std::{
    ffi::{CStr, CString, c_char},
    sync::{Arc, Mutex, OnceLock},
};

static CLIENT: Mutex<Option<Arc<MobileClient>>> = Mutex::new(None);
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("mobile Tokio runtime")
    })
}

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
    std::panic::catch_unwind(|| {
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
        let (Some(profile), Some(email), Some(password), Some(identity)) =
            (read(profile), read(email), read(password), read(identity))
        else {
            return 0;
        };
        let mut slot = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|client| !client.finished()) {
            return 0;
        }
        let client = Arc::new(MobileClient::default());
        runtime().spawn(client.clone().run(profile, email, password, identity));
        *slot = Some(client);
        1
    })
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
    }
    #[test]
    fn invalid_profile_finishes_without_network_or_credentials_in_output() {
        let profile = CString::new("{}").unwrap();
        let email = CString::new("test@example.invalid").unwrap();
        let password = CString::new("never-log-this-test-password").unwrap();
        let path = CString::new("/unused").unwrap();
        assert_eq!(
            unsafe {
                spatial_mobile_start(
                    profile.as_ptr(),
                    email.as_ptr(),
                    password.as_ptr(),
                    path.as_ptr(),
                )
            },
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
