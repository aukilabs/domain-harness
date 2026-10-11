//! Phone joystick relay. A phone posts normalized stick values for one robot;
//! that robot reads its own latest command with an age measured by Home. Home
//! moves no motors and applies no speeds: the robot decides who may drive it,
//! how fast, and stops whenever its input goes stale.
use anyhow::{Result, anyhow, ensure};
use auki_components::ContractType;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub const COMPONENT: &str = "home-phone-drive";
/// A command older than this is never handed to a robot.
pub const MAX_AGE: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// One joystick session; sequences only order commands within it.
    pub session: String,
    pub sequence: u64,
    /// Peer ID of the robot this command is for.
    pub target_peer_id: String,
    /// Stick deflection, -1 (back) to 1 (forward).
    pub forward: f32,
    /// Stick deflection, -1 (turn right) to 1 (turn left).
    pub turn: f32,
}
impl ContractType for Command {
    const DATATYPE: &'static str = "spatial.phone-drive/v1";
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadDrive {
    pub frame: String,
}
impl ContractType for ReadDrive {
    const DATATYPE: &'static str = "spatial.phone-drive-read/v1";
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Latest {
    /// Transport identity of the phone that sent it, as authenticated by Home.
    pub driver_peer_id: String,
    pub session: String,
    pub sequence: u64,
    pub forward: f32,
    pub turn: f32,
    /// Time since Home received it, on Home's own clock.
    pub age_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub authority_peer_id: String,
    pub frame: String,
    /// The calling robot's own latest command, if one is fresh.
    pub command: Option<Latest>,
}
impl ContractType for Snapshot {
    const DATATYPE: &'static str = "spatial.phone-drive-snapshot/v1";
}

/// Latest command per robot. One phone holds a robot until it goes quiet.
#[derive(Default)]
pub struct Board {
    latest: BTreeMap<String, (String, Command, Instant)>,
}
impl Board {
    pub fn accept(&mut self, driver: &str, command: Command, now: Instant) -> Result<bool> {
        ensure!(!driver.is_empty(), "Missing driver identity");
        uuid::Uuid::parse_str(&command.session)?;
        ensure!(
            !command.target_peer_id.is_empty() && command.target_peer_id.len() <= 128,
            "Invalid drive target"
        );
        ensure!(
            command.forward.is_finite()
                && command.turn.is_finite()
                && command.forward.abs() <= 1.
                && command.turn.abs() <= 1.,
            "Stick values must be within -1 to 1"
        );
        self.latest
            .retain(|_, (_, _, at)| now.duration_since(*at) < MAX_AGE);
        if let Some((holder, old, _)) = self.latest.get(&command.target_peer_id) {
            ensure!(holder == driver, "Another device is driving this robot");
            if old.session == command.session && command.sequence <= old.sequence {
                return Ok(false);
            }
        }
        ensure!(
            self.latest.contains_key(&command.target_peer_id) || self.latest.len() < 64,
            "Drive relay capacity reached"
        );
        self.latest.insert(
            command.target_peer_id.clone(),
            (driver.into(), command, now),
        );
        Ok(true)
    }
    /// What `robot` should act on now. Stale commands are withheld, not zeroed:
    /// absence is the stop signal.
    pub fn read(&self, robot: &str, now: Instant) -> Option<Latest> {
        let (driver, command, at) = self.latest.get(robot)?;
        let age = now.duration_since(*at);
        (age < MAX_AGE).then(|| Latest {
            driver_peer_id: driver.clone(),
            session: command.session.clone(),
            sequence: command.sequence,
            forward: command.forward,
            turn: command.turn,
            age_ms: age.as_millis() as u64,
        })
    }
}

async fn target(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
) -> Result<(
    auki_component_protocol::ComponentProtocolClient,
    auki_sdk::PeerId,
    auki_sdk::Multiaddr,
    auki_components::ComponentReference,
)> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    for route in &home.routes {
        let route: auki_sdk::Multiaddr = route.parse()?;
        let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = tokio::time::timeout(
            Duration::from_secs(1),
            client.catalog_exact(expected, route.clone(), None),
        )
        .await
        else {
            continue;
        };
        let entry = snapshot
            .components
            .iter()
            .find(|c| c.manifest.component_id == home.scoped_component(COMPONENT))
            .ok_or_else(|| anyhow!("Home drive relay unavailable"))?;
        ensure!(
            entry.manifest.peer_id == home.peer_id,
            "Drive relay authority mismatch"
        );
        return Ok((client, expected, route, entry.manifest.reference()));
    }
    anyhow::bail!("Home drive relay unreachable")
}

/// Post one stick command (used by tests and tools; the phone app has its own loop).
pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    command: &Command,
) -> Result<bool> {
    let (client, expected, route, reference) = target(peer, home).await?;
    Ok(client
        .invoke_exact::<_, bool>(
            expected,
            route,
            reference,
            "update",
            "phone-drive",
            uuid::Uuid::new_v4().to_string(),
            Some(Duration::from_millis(500)),
            command,
        )
        .await?
        .result)
}

/// Robot-side reader. Discovery is cached; every command read is a fresh,
/// authenticated invocation whose round trip bounds its transit age.
#[derive(Default)]
pub struct Reader {
    pinned: Option<(
        String,
        auki_component_protocol::ComponentProtocolClient,
        auki_sdk::PeerId,
        auki_sdk::Multiaddr,
        auki_components::ComponentReference,
    )>,
}
impl Reader {
    /// False until discovery has found Home's relay. Discovery walks every
    /// route and needs far longer than a pinned command read.
    pub fn is_pinned(&self) -> bool {
        self.pinned.is_some()
    }
    pub async fn read(
        &mut self,
        peer: &auki_sdk::AukiPeer,
        home: &crate::network::HomeConnection,
    ) -> Result<(Snapshot, Duration)> {
        let key = serde_json::to_string(home)?;
        if self.pinned.as_ref().is_some_and(|p| p.0 != key) {
            self.pinned = None;
        }
        if self.pinned.is_none() {
            let (client, expected, route, reference) = target(peer, home).await?;
            self.pinned = Some((key, client, expected, route, reference));
        }
        let (_, client, expected, route, reference) = self.pinned.as_ref().unwrap();
        let began = Instant::now();
        let reply = client
            .invoke_exact::<_, Snapshot>(
                *expected,
                route.clone(),
                reference.clone(),
                "snapshot",
                "robot-drive",
                uuid::Uuid::new_v4().to_string(),
                Some(Duration::from_millis(300)),
                &ReadDrive {
                    frame: home.frame.clone(),
                },
            )
            .await;
        let elapsed = began.elapsed();
        let snapshot = match reply {
            Ok(reply) => reply.result,
            Err(error) => {
                self.pinned = None;
                return Err(error.into());
            }
        };
        if snapshot.authority_peer_id != home.peer_id || snapshot.frame != home.frame {
            self.pinned = None;
            anyhow::bail!("Drive command belongs to another Home or scene");
        }
        Ok((snapshot, elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(session: &str, sequence: u64, forward: f32) -> Command {
        Command {
            session: session.into(),
            sequence,
            target_peer_id: "robot".into(),
            forward,
            turn: 0.,
        }
    }
    #[test]
    fn one_driver_holds_a_robot_and_stale_commands_are_withheld() -> Result<()> {
        let mut board = Board::default();
        let now = Instant::now();
        let session = uuid::Uuid::new_v4().to_string();
        assert!(board.read("robot", now).is_none());
        assert!(board.accept("phone-a", command(&session, 1, 0.5), now)?);
        assert!(
            !board.accept("phone-a", command(&session, 1, 0.9), now)?,
            "replay ignored"
        );
        let seen = board
            .read("robot", now + Duration::from_millis(200))
            .unwrap();
        assert_eq!(
            (seen.driver_peer_id.as_str(), seen.forward, seen.age_ms),
            ("phone-a", 0.5, 200)
        );
        assert!(
            board.read("other-robot", now).is_none(),
            "a robot reads only its own command"
        );
        assert!(
            board
                .accept(
                    "phone-b",
                    command(&uuid::Uuid::new_v4().to_string(), 1, 1.),
                    now
                )
                .is_err(),
            "a second phone cannot take over a held robot"
        );
        assert!(
            board.read("robot", now + MAX_AGE).is_none(),
            "silence is the stop signal"
        );
        let later = now + MAX_AGE;
        assert!(board.accept(
            "phone-b",
            command(&uuid::Uuid::new_v4().to_string(), 1, 1.),
            later
        )?);
        for bad in [f32::NAN, 1.5, -2.] {
            assert!(
                board
                    .accept("phone-b", command(&session, 9, bad), later)
                    .is_err()
            );
        }
        Ok(())
    }
}
