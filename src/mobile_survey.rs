//! Compact connectivity samples at phone pose. Local JSONL on the phone is the archive.
use crate::model::FRAME;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

pub const COMPONENT: &str = "home-connectivity-survey";
pub const SCHEMA: &str = "connectivity-survey.samples/v1";
const CAPACITY: usize = 1024;
const WIFI_LIMIT: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifiRow {
    pub ssid: String,
    pub bssid: String,
    pub rssi_dbm: i32,
    pub frequency_mhz: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cellular {
    pub radio: String,
    #[serde(default)]
    pub dbm: Option<i32>,
    #[serde(default)]
    pub level: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PathSide {
    #[serde(default)]
    pub rtt_ms: Option<u32>,
    pub loss_pct: u8,
    #[serde(default)]
    pub window: u8,
    #[serde(default)]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssid: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PathQuality {
    #[serde(default)]
    pub wifi: Option<PathSide>,
    #[serde(default)]
    pub cellular: Option<PathSide>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub id: String,
    pub captured_at: String,
    #[serde(default)]
    pub scan_age_ms: u64,
    pub translation: [f64; 3],
    #[serde(default)]
    pub wifi: Vec<WifiRow>,
    pub cellular: Cellular,
    #[serde(default)]
    pub path: Option<PathQuality>,
}

impl auki_components::ContractType for Sample {
    const DATATYPE: &'static str = "connectivity-survey.sample/v1";
}

#[derive(Default)]
pub struct Log {
    samples: Vec<Sample>,
}

impl Log {
    pub fn accept(&mut self, mut sample: Sample) -> Result<()> {
        ensure!(
            !sample.id.is_empty() && sample.id.len() <= 128,
            "Invalid survey sample id"
        );
        ensure!(
            sample.translation.iter().all(|n| n.is_finite()),
            "Invalid survey pose"
        );
        sample.wifi = compact_wifi(sample.wifi);
        sample.cellular.radio = truncate_bytes(&sample.cellular.radio, 16);
        for row in &mut sample.wifi {
            row.ssid = truncate_bytes(&row.ssid, 32);
            row.bssid = truncate_bytes(&row.bssid, 32);
            row.rssi_dbm = row.rssi_dbm.clamp(-127, 0);
        }
        if let Some(path) = sample.path.take() {
            let sanitized = sanitize_path(path);
            if sanitized.wifi.is_some() || sanitized.cellular.is_some() {
                sample.path = Some(sanitized);
            }
        }
        if self.samples.iter().any(|existing| existing.id == sample.id) {
            return Ok(());
        }
        if self.samples.len() >= CAPACITY {
            self.samples.remove(0);
        }
        self.samples.push(sample);
        Ok(())
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "frame": FRAME,
            "samples": self.samples,
        })
    }
}

pub fn sync(store: &mut crate::model::Store, log: &Log) {
    let value = log.snapshot();
    if store.built.registries["connectivity_survey"] != value {
        store.built.registries["connectivity_survey"] = value;
    }
}

fn truncate_bytes(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let end = value
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|end| *end <= max)
        .last()
        .unwrap_or(0);
    value[..end].to_string()
}

pub fn append_samples(view: &mut Value, registry: &Value) {
    if registry["frame"] != FRAME {
        return;
    }
    view["survey_samples"] = registry["samples"].clone();
}

#[derive(Deserialize)]
struct PhoneVec {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize)]
struct PhonePose {
    translation: PhoneVec,
}

#[derive(Deserialize)]
struct PhoneLine {
    #[serde(default)]
    id: Option<String>,
    captured_at: String,
    #[serde(default)]
    scan_age_ms: u64,
    pose: PhonePose,
    #[serde(default)]
    wifi: Vec<WifiRow>,
    cellular: Cellular,
    #[serde(default)]
    path: Option<PathQuality>,
}

fn compact_wifi(wifi: Vec<WifiRow>) -> Vec<WifiRow> {
    let mut strongest = Vec::<WifiRow>::new();
    for row in wifi {
        if let Some(existing) = strongest.iter_mut().find(|existing| existing.ssid == row.ssid)
        {
            if row.rssi_dbm > existing.rssi_dbm {
                *existing = row;
            }
        } else {
            strongest.push(row);
        }
    }
    strongest.sort_by(|a, b| b.rssi_dbm.cmp(&a.rssi_dbm));
    strongest.truncate(WIFI_LIMIT);
    strongest
}

fn from_phone(line: PhoneLine, fallback_id: String) -> Sample {
    Sample {
        id: line
            .id
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .unwrap_or(fallback_id),
        captured_at: line.captured_at,
        scan_age_ms: line.scan_age_ms,
        translation: [
            line.pose.translation.x,
            line.pose.translation.y,
            line.pose.translation.z,
        ],
        wifi: compact_wifi(line.wifi),
        cellular: line.cellular,
        path: line.path,
    }
}

fn sanitize_path(mut path: PathQuality) -> PathQuality {
    path.wifi = path.wifi.take().map(sanitize_side);
    path.cellular = path.cellular.take().map(sanitize_side);
    path
}

fn sanitize_side(mut side: PathSide) -> PathSide {
    side.loss_pct = side.loss_pct.min(100);
    side.window = side.window.min(10);
    side.target = truncate_bytes(&side.target, 32);
    side.ssid = side
        .ssid
        .take()
        .map(|ssid| truncate_bytes(&ssid, 32));
    side
}

/// Parse phone JSONL (`connectivity-surveyor.sample.v1`) or a compact overlay JSON array.
pub fn import_text(body: &str) -> Result<Vec<Sample>> {
    let trimmed = body.trim();
    ensure!(!trimmed.is_empty(), "Survey import is empty");
    if trimmed.starts_with('[') {
        let samples: Vec<Sample> = serde_json::from_str(trimmed)?;
        return Ok(samples
            .into_iter()
            .map(|mut sample| {
                sample.wifi = compact_wifi(sample.wifi);
                sample
            })
            .collect());
    }
    let mut samples = Vec::new();
    for (index, line) in trimmed.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(mut sample) = serde_json::from_str::<Sample>(line) {
            sample.wifi = compact_wifi(sample.wifi);
            samples.push(sample);
            continue;
        }
        let phone: PhoneLine = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("Invalid survey line {}: {e}", index + 1))?;
        samples.push(from_phone(phone, format!("import-{index}")));
    }
    ensure!(!samples.is_empty(), "Survey import is empty");
    Ok(samples)
}

pub type SurveyTarget = (auki_sdk::Multiaddr, auki_components::ComponentReference);

async fn invoke(
    client: &auki_component_protocol::ComponentProtocolClient,
    expected: auki_sdk::PeerId,
    route: auki_sdk::Multiaddr,
    component: auki_components::ComponentReference,
    sample: &Sample,
) -> Result<()> {
    client
        .invoke_exact::<_, bool>(
            expected,
            route,
            component,
            "ingest",
            "survey-sample",
            uuid::Uuid::new_v4().to_string(),
            Some(Duration::from_secs(5)),
            sample,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Home survey ingest failed: {e}"))?;
    Ok(())
}

pub async fn send(
    peer: &auki_sdk::AukiPeer,
    home: &crate::network::HomeConnection,
    sample: &Sample,
    cache: &mut Option<SurveyTarget>,
) -> Result<()> {
    use auki_component_protocol::{CatalogResponse, ComponentProtocolClient};
    let expected = home.validate(peer)?;
    let client = ComponentProtocolClient::new(peer.protocols());
    if let Some((route, component)) = cache.clone() {
        if invoke(&client, expected, route, component, sample)
            .await
            .is_ok()
        {
            return Ok(());
        }
        *cache = None;
    }
    let mut saw_catalog = false;
    let mut missing_component = false;
    for attempt in 0..3 {
        for route in &home.routes {
            let route: auki_sdk::Multiaddr = route.parse()?;
            let snapshot = match tokio::time::timeout(
                Duration::from_secs(5),
                client.catalog_exact(expected, route.clone(), None),
            )
            .await
            {
                Ok(Ok(CatalogResponse::Snapshot { snapshot })) => {
                    saw_catalog = true;
                    snapshot
                }
                _ => continue,
            };
            let Some(entry) = snapshot
                .components
                .iter()
                .find(|c| c.manifest.component_id == COMPONENT)
            else {
                missing_component = true;
                continue;
            };
            let component = entry.manifest.reference();
            invoke(&client, expected, route.clone(), component.clone(), sample).await?;
            *cache = Some((route, component));
            return Ok(());
        }
        if attempt + 1 < 3 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }
    if missing_component {
        anyhow::bail!("Home catalog has no home-connectivity-survey; restart spatial-home");
    }
    if saw_catalog {
        anyhow::bail!("Home catalog has no home-connectivity-survey; restart spatial-home");
    }
    anyhow::bail!("Home catalog timed out looking up connectivity survey")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> Sample {
        Sample {
            id: id.into(),
            captured_at: "2026-10-14T02:00:00Z".into(),
            scan_age_ms: 800,
            translation: [1.5, 1.2, -2.0],
            wifi: vec![WifiRow {
                ssid: "Auki-Office".into(),
                bssid: "aa:bb:cc:dd:ee:ff".into(),
                rssi_dbm: -47,
                frequency_mhz: 5180,
            }],
            cellular: Cellular {
                radio: "NR".into(),
                dbm: Some(-89),
                level: Some(3),
            },
            path: None,
        }
    }

    #[test]
    fn keeps_camera_height_and_caps_log() {
        let mut log = Log::default();
        log.accept(sample("one")).unwrap();
        assert_eq!(log.snapshot()["samples"][0]["translation"][1], 1.2);
        for i in 0..CAPACITY {
            log.accept(sample(&format!("s{i}"))).unwrap();
        }
        assert_eq!(log.samples.len(), CAPACITY);
        assert_ne!(log.samples[0].id, "one");
    }

    #[test]
    fn truncates_overlong_wifi_list_and_keeps_strongest_ssid() {
        let mut row = sample("big");
        row.wifi = (0..WIFI_LIMIT + 4)
            .map(|i| WifiRow {
                ssid: format!("n{i}"),
                bssid: format!("b{i}"),
                rssi_dbm: -50,
                frequency_mhz: 2412,
            })
            .collect();
        row.wifi.push(WifiRow {
            ssid: "n0".into(),
            bssid: "mesh".into(),
            rssi_dbm: -20,
            frequency_mhz: 5180,
        });
        let mut log = Log::default();
        log.accept(row).unwrap();
        let snapshot = log.snapshot();
        let wifi = snapshot["samples"][0]["wifi"].as_array().unwrap();
        assert_eq!(wifi.len(), WIFI_LIMIT);
        assert_eq!(wifi[0]["ssid"], "n0");
        assert_eq!(wifi[0]["rssi_dbm"], -20);
        assert_eq!(wifi[0]["bssid"], "mesh");
    }

    #[test]
    fn truncates_long_ssid_and_clamps_rssi() {
        let mut row = sample("wide");
        row.wifi[0].ssid = "网".repeat(20);
        row.wifi[0].rssi_dbm = -140;
        let mut log = Log::default();
        log.accept(row).unwrap();
        let wifi = &log.snapshot()["samples"][0]["wifi"][0];
        assert!(wifi["ssid"].as_str().unwrap().len() <= 32);
        assert_eq!(wifi["rssi_dbm"], -127);
    }

    #[test]
    fn import_phone_jsonl_truncates_wifi() {
        let mut wifi = String::new();
        for i in 0..20 {
            if i > 0 {
                wifi.push(',');
            }
            wifi.push_str(&format!(
                r#"{{"ssid":"n{i}","bssid":"aa:bb:cc:dd:ee:{i:02}","rssi_dbm":{},"frequency_mhz":2412}}"#,
                -40 - i
            ));
        }
        let line = format!(
            r#"{{"schema":"connectivity-surveyor.sample.v1","captured_at":"2026-10-14T05:03:00Z","scan_age_ms":12,"trigger":"moved","pose":{{"translation":{{"x":1.5,"y":1.2,"z":-2.0}},"orientation":{{"x":0.0,"y":0.0,"z":0.0,"w":1.0}}}},"wifi":[{wifi}],"cellular":{{"radio":"unknown","dbm":null,"level":null}}}}"#
        );
        let samples = import_text(&line).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].translation[1], 1.2);
        assert_eq!(samples[0].wifi.len(), 20);
        assert_eq!(samples[0].wifi[0].ssid, "n0");
        assert_eq!(samples[0].path, None);
    }

    #[test]
    fn import_phone_jsonl_keeps_path_metrics() {
        let line = r#"{"schema":"connectivity-surveyor.sample.v1","captured_at":"2026-10-14T05:03:00Z","pose":{"translation":{"x":1.5,"y":1.2,"z":-2.0},"orientation":{"x":0.0,"y":0.0,"z":0.0,"w":1.0}},"wifi":[],"cellular":{"radio":"NR","dbm":-89},"path":{"wifi":{"rtt_ms":18,"loss_pct":0,"window":10,"target":"8.8.8.8","ssid":"Auki-Office"},"cellular":{"rtt_ms":42,"loss_pct":10,"window":10,"target":"8.8.8.8"}}}"#;
        let samples = import_text(line).unwrap();
        let path = samples[0].path.as_ref().unwrap();
        assert_eq!(path.wifi.as_ref().unwrap().rtt_ms, Some(18));
        assert_eq!(path.wifi.as_ref().unwrap().ssid.as_deref(), Some("Auki-Office"));
        assert_eq!(path.cellular.as_ref().unwrap().loss_pct, 10);
    }
}
