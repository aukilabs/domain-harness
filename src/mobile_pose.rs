//! Ephemeral, calibrated phone presence. Transport identity owns each pose.
use crate::model::FRAME;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub const COMPONENT: &str = "home-phone-presence";
/// Local addition: display names per phone peer, read once from
/// `SPATIAL_PHONE_NAMES` (default `data/phone-names.json`, `{"<peer id>": "<name>"}`).
/// The entry `"*"` names every unlisted phone; otherwise a short peer suffix is used.
static NAMES: std::sync::LazyLock<BTreeMap<String, String>> = std::sync::LazyLock::new(|| {
    let path = std::env::var("SPATIAL_PHONE_NAMES").unwrap_or_else(|_| "data/phone-names.json".into());
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
});
pub fn display_name(peer: &str) -> String {
    NAMES.get(peer).or_else(|| NAMES.get("*")).cloned().unwrap_or_else(|| {
        let tail = &peer[peer.len().saturating_sub(4)..];
        format!("phone {tail}")
    })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub session: String,
    pub timestamp_ns: u64,
    pub pose: auki_scenegraph::RigidTransform,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub sample: Option<Sample>,
}
impl auki_components::ContractType for Update {
    const DATATYPE: &'static str = "spatial.phone-presence/v1";
}
#[derive(Default)]
pub struct Presence {
    entries: BTreeMap<String, (Sample, Instant, bool)>,
}
impl Presence {
    pub fn accept(&mut self, peer: &str, update: Update, now: Instant) -> Result<()> {
        self.accept_in_frame(peer, update, now, FRAME)
    }
    pub fn accept_in_frame(
        &mut self,
        peer: &str,
        update: Update,
        now: Instant,
        frame: &str,
    ) -> Result<()> {
        ensure!(!peer.is_empty(), "Missing phone identity");
        let Some(sample) = update.sample else {
            if let Some(entry) = self.entries.get_mut(peer) {
                entry.2 = false;
            }
            return Ok(());
        };
        uuid::Uuid::parse_str(&sample.session)?;
        ensure!(
            sample.pose.to_frame_id == frame
                && sample.pose.from_frame_id == format!("phone-camera:{}", sample.session),
            "Phone pose frame mismatch"
        );
        ensure!(
            sample
                .pose
                .translation
                .iter()
                .chain(sample.pose.rotation_wxyz.iter())
                .all(|n| n.is_finite())
                && (sample.pose.rotation_wxyz.iter().map(|v| v * v).sum::<f64>() - 1.).abs()
                    < 0.001,
            "Invalid phone pose"
        );
        if let Some((old, _, _)) = self.entries.get(peer)
            && old.session == sample.session
            && sample.timestamp_ns <= old.timestamp_ns
        {
            return Ok(());
        }
        ensure!(
            self.entries.contains_key(peer) || self.entries.len() < 64,
            "Phone presence capacity reached"
        );
        self.entries.insert(peer.into(), (sample, now, true));
        Ok(())
    }
    pub fn snapshot(&self, now: Instant) -> Value {
        self.snapshot_in_frame(now, FRAME)
    }
    pub fn snapshot_in_frame(&self, now: Instant, frame: &str) -> Value {
        let poses: Vec<_> = self.entries.iter().filter(|(_, (_,received,active))| *active && now.duration_since(*received) < Duration::from_secs(3)).map(|(peer,(s,received,_))| {
            let mut clock = crate::model::clock_entry(peer, &s.session);
            clock.clock_id = "arkit-capture".into();
            json!({"peer_id":peer,"name":display_name(peer),"pose":s.pose,"capture_timestamp_ns":s.timestamp_ns,"clock":clock,"received_age_ms":now.duration_since(*received).as_millis()})
        }).collect();
        json!({"frame":frame,"poses":poses})
    }
}
pub fn sync(store: &mut crate::model::Store, presence: &Presence, now: Instant) {
    let value = presence.snapshot(now);
    if store.built.registries["mobile_presence"] != value {
        store.built.registries["mobile_presence"] = value;
    }
}
pub fn append_labels(view: &mut Value, registry: &Value) {
    append_labels_in_frame(view, registry, FRAME)
}
pub fn append_labels_in_frame(view: &mut Value, registry: &Value, frame: &str) {
    if registry["frame"] != frame {
        return;
    }
    let Some(poses) = registry["poses"].as_array() else {
        return;
    };
    if !view["notes"].is_array() {
        view["notes"] = json!([]);
    }
    for pose in poses {
        view["notes"].as_array_mut().unwrap().push(json!({
            "id":format!("phone-name:{}",pose["peer_id"].as_str().unwrap_or("")),
            "text":pose["name"].as_str().unwrap_or("phone"), "position":pose["pose"]["translation"], "height_m":0,
            "phone_pose":pose
        }));
    }
}

pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    update: &Update,
) -> Result<()> {
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
            .ok_or_else(|| anyhow::anyhow!("Home phone presence unavailable"))?;
        client
            .invoke_exact::<_, bool>(
                expected,
                route,
                entry.manifest.reference(),
                "update",
                "phone-pose",
                uuid::Uuid::new_v4().to_string(),
                Some(Duration::from_secs(1)),
                update,
            )
            .await?;
        return Ok(());
    }
    anyhow::bail!("Home phone presence unavailable")
}
