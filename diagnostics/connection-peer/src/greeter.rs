//! `greeter`: say hello through the robot's speaker, now, where it stands.
//! The words come from piper (offline text to speech) and play through BBOS's
//! speaker daemon, both in `scripts/diagnostic-greet.py`. One greeting at a time.
use crate::greet::{Greet, GreetAccepted, COMPONENT, OPERATION, contract};
use anyhow::Result;
use auki_components::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

/// piper and its voice, as installed on the robot (see the README).
fn voice() -> (std::path::PathBuf, std::path::PathBuf) {
    let tools = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools");
    (tools.join("piper/piper"), tools.join("piper-voices/en_US-lessac-medium.onnx"))
}

pub struct Greeter {
    _component: Component,
}
impl Greeter {
    pub fn mount(p: &crate::poses::Publisher) -> Result<Self> {
        let (piper, model) = voice();
        let speaks = piper.is_file() && model.is_file();
        if !speaks {
            crate::diagnostic_log!("Greeter: piper or its voice is missing under ~/tools; greetings are refused");
        }
        let speaking = Arc::new(AtomicBool::new(false));
        let component = p.runtime.component(ComponentSpec::new(COMPONENT).operable(contract()))?;
        let operation = component.operable(
            OPERATION,
            // Any authenticated peer, like point_at: a greeting moves nothing.
            |ctx| !ctx.caller_peer_id.is_empty(),
            move |ctx, request: Greet| {
                let rejected = |e: String| InvocationError::Rejected(e);
                request.validate().map_err(|e| rejected(e.to_string()))?;
                if !speaks {
                    return Err(rejected("This robot has no voice: install piper under ~/tools".into()));
                }
                if speaking.swap(true, Ordering::SeqCst) {
                    return Err(rejected("Already greeting someone".into()));
                }
                let text = request.text();
                crate::diagnostic_log!("greet {} from {}: {text}", request.request_id, ctx.caller_peer_id);
                let (done, line) = (speaking.clone(), serde_json::json!({"text": text}).to_string());
                std::thread::spawn(move || {
                    // Words go over stdin, never as a command or an argument.
                    let spoken = Command::new("python3").arg("scripts/diagnostic-greet.py")
                        .env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
                        .env_remove("AUKI_EMAIL").env_remove("AUKI_PASSWORD")
                        .stdin(Stdio::piped()).stdout(Stdio::null()).spawn()
                        .and_then(|mut child| {
                            child.stdin.take().unwrap().write_all(line.as_bytes())?;
                            child.wait()
                        });
                    match spoken {
                        Ok(status) if status.success() => {}
                        Ok(status) => crate::diagnostic_log!("Greeter: speech failed ({status})"),
                        Err(e) => crate::diagnostic_log!("Greeter: speech failed: {e}"),
                    }
                    done.store(false, Ordering::SeqCst);
                });
                Ok(GreetAccepted { request_id: request.request_id, speaks, text })
            },
        )?;
        component.expose()?;
        p.endpoint.export_operable(&operation)?;
        Ok(Self { _component: component })
    }
}
