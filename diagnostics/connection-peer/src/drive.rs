//! Teleop from Home. Follows the paired scene's drive stream exactly as the
//! full runner does, keeps only commands for this robot from its Home peer, and
//! hands them to the fail-closed BBOS drive helper (`drive-bracketbot.py`).
//! Home moves nothing and applies no speeds: the helper decides the twist,
//! stops on stale input, and only holds `drive.ctrl` while commands are fresh.
use crate::drive_contract::{Pushed, pushed_size, stream_limits, stream_product};
use anyhow::{Context, Result, anyhow, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationStart, RemoteObservationEvent,
};
use auki_components::ProductReference;
use auki_sdk::{Multiaddr, PeerId};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub const STATE: &str = "drive";
/// The helper treats a pushed command older than 400 ms as a stop; refresh the
/// cache on silence so it sees "no command", never a stale file.
const SILENCE: Duration = Duration::from_millis(250);

/// The local scene a Home profile pairs with, as Home's `frame_scene_id`.
pub fn scene_id(frame: &str) -> Option<&str> {
    let rest = frame.strip_prefix("local-scene:")?;
    let id = rest.split(':').next()?;
    let suffix = &rest[id.len()..];
    let valid = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (valid && (suffix == ":origin" || suffix.starts_with(":portal-") || suffix.starts_with(":epoch-")))
        .then_some(id)
}

/// The helper's input: `spatial.robot-drive-input/v1`, as the full runner writes it.
pub fn packet(home: &str, command: Option<&Pushed>, received_ns: u64) -> serde_json::Value {
    serde_json::json!({
        "schema": "spatial.robot-drive-input/v1", "authenticated": true, "transport": "push",
        "allowed_driver_peer_id": home, "request_age_ms": 0, "received_ns": received_ns,
        "command": command.map(|c| serde_json::json!({
            "driver_peer_id": c.driver_peer_id, "session": c.session, "sequence": c.sequence,
            "forward": c.forward, "turn": c.turn, "age_ms": 0})),
    })
}

/// Only Home's own peer may drive, and only this robot's commands count.
pub fn admitted(c: &Pushed, own: &str, home: &str, latest: &mut (String, u64)) -> bool {
    if c.target_peer_id != own || c.driver_peer_id != home {
        return false;
    }
    if latest.0 == c.session && c.sequence <= latest.1 {
        return false;
    }
    *latest = (c.session.clone(), c.sequence);
    true
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn save(path: &Path, value: &serde_json::Value) -> Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

/// Follow Home's drive stream until stopped, rewriting the helper's input.
pub async fn follow(
    client: ComponentProtocolClient,
    home: PeerId,
    routes: Vec<Multiaddr>,
    scene: String,
    own: String,
) {
    let path = PathBuf::from(STATE).join("drive-command.json");
    let home_id = home.to_string();
    let product = stream_product(&scene);
    let mut last_error = String::new();
    loop {
        let _ = save(&path, &packet(&home_id, None, now_ns()));
        let attempt = async {
            for route in &routes {
                let found = async {
                    let CatalogResponse::Snapshot { snapshot } =
                        client.catalog_exact(home, route.clone(), None).await?
                    else {
                        anyhow::bail!("Missing Home catalog")
                    };
                    let entry = snapshot
                        .products
                        .iter()
                        .find(|p| p.manifest.product_id == product)
                        .ok_or_else(|| anyhow!("Home has no drive stream for scene {scene}"))?;
                    ensure!(entry.manifest.peer_id == home_id, "Drive stream owner mismatch");
                    let reference = ProductReference {
                        peer_id: home_id.clone(),
                        product_id: product.clone(),
                        manifest_hash: entry.manifest_hash.clone(),
                    };
                    // Never replay an old stick position.
                    Ok(client
                        .subscribe_product_exact(home, route.clone(), reference, ObservationStart::NewOnly,
                            stream_limits(), pushed_size)
                        .await?)
                };
                if let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(3), found).await {
                    return Ok(stream);
                }
            }
            anyhow::bail!("Home drive stream unavailable")
        };
        let mut stream = match attempt.await {
            Ok(stream) => stream,
            Err(e) => {
                let error = e.to_string();
                if error != last_error {
                    crate::diagnostic_log!("Drive relay: {error}");
                    last_error = error;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        crate::diagnostic_log!("Drive relay: following Home's commands for scene {scene}");
        last_error.clear();
        let mut latest = (String::new(), 0u64);
        loop {
            let event = tokio::time::timeout(SILENCE, stream.next()).await;
            let command = match event {
                Ok(Ok(Some(RemoteObservationEvent::Observation(o)))) => {
                    if !admitted(&o.payload, &own, &home_id, &mut latest) {
                        continue;
                    }
                    Some(o.payload.as_ref().clone())
                }
                Ok(Ok(Some(RemoteObservationEvent::Gap(_)))) | Err(_) => None,
                _ => break,
            };
            if let Err(e) = save(&path, &packet(&home_id, command.as_ref(), now_ns())) {
                crate::diagnostic_log!("Drive command cache: {e}");
            }
        }
        let _ = stream.close().await;
        crate::diagnostic_log!("Drive relay: Home's stream ended; resubscribing");
    }
}

/// The BBOS drive helper, at full teleop speeds, allowing only `home`.
pub struct Helper(Child);
impl Helper {
    pub fn start(home: &str) -> Result<Self> {
        std::fs::create_dir_all(STATE)?;
        save(&Path::new(STATE).join("drive-command.json"), &packet(home, None, now_ns()))?;
        let child = Command::new("python3")
            .args(["scripts/bracketbot-python.py", "scripts/drive-bracketbot.py", "--state", STATE,
                "--driver-peer", home, "--execute", "--fast"])
            .env_remove("AUKI_ACCESS_KEY")
            .env_remove("AUKI_APP_SECRET")
            .env_remove("AUKI_EMAIL")
            .env_remove("AUKI_PASSWORD")
            .stdin(Stdio::null())
            .spawn()
            .context("Drive helper did not start")?;
        Ok(Self(child))
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        // SIGTERM lets the helper write a final stop and release drive.ctrl.
        let _ = Command::new("kill").args(["-TERM", &self.0.id().to_string()]).status();
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if !matches!(self.0.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(target: &str, driver: &str, session: &str, sequence: u64) -> Pushed {
        Pushed {
            driver_peer_id: driver.into(),
            target_peer_id: target.into(),
            session: session.into(),
            sequence,
            forward: 0.5,
            turn: -0.25,
        }
    }
    #[test]
    fn only_home_may_drive_this_robot_and_replays_are_dropped() {
        let mut latest = (String::new(), 0);
        assert!(admitted(&command("robot", "home", "s", 1), "robot", "home", &mut latest));
        assert!(!admitted(&command("robot", "home", "s", 1), "robot", "home", &mut latest));
        assert!(admitted(&command("robot", "home", "s", 2), "robot", "home", &mut latest));
        // A phone, or a command for another robot, is ignored.
        assert!(!admitted(&command("robot", "phone", "s", 3), "robot", "home", &mut latest));
        assert!(!admitted(&command("other", "home", "s", 3), "robot", "home", &mut latest));
        // A new session restarts sequences.
        assert!(admitted(&command("robot", "home", "t", 1), "robot", "home", &mut latest));
    }
    #[test]
    fn the_helper_input_matches_the_full_runner() {
        let p = packet("home", Some(&command("robot", "home", "s", 7)), 42);
        assert_eq!(p["schema"], "spatial.robot-drive-input/v1");
        assert_eq!(p["transport"], "push");
        assert_eq!(p["allowed_driver_peer_id"], "home");
        assert_eq!(p["command"]["driver_peer_id"], "home");
        assert_eq!(p["command"]["sequence"], 7);
        assert!(packet("home", None, 1)["command"].is_null());
    }
    #[test]
    fn scene_ids_follow_home_profiles() {
        assert_eq!(scene_id("local-scene:humanoid-hub-conference:portal-HNY3JGNLRAE"), Some("humanoid-hub-conference"));
        assert_eq!(scene_id("local-scene:bb-lab:origin"), Some("bb-lab"));
        assert_eq!(scene_id("domain:b2881524:home-origin"), None);
        assert_eq!(scene_id("local-scene:Bad_Id:origin"), None);
    }
}
