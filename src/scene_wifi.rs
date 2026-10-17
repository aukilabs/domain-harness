//! Scene-owned onboarding configuration, separate from spatial snapshots.
//! Credentials are disclosed only to explicitly permitted onboarding hosts.
use anyhow::{Result, anyhow, ensure};
use auki_components::*;
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, io::Write, path::{Path, PathBuf}, sync::{Arc, Mutex}};
use crate::onboarding::Wifi;

pub const COMPONENT: &str = "scene-wifi";
pub const OPERATION: &str = "get_for_onboarding";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    wifi: Option<Wifi>,
    allowed_peer_ids: BTreeSet<String>,
}
fn validate_peers(peers: &BTreeSet<String>) -> Result<()> {
    ensure!(peers.len() <= 64 && peers.iter().all(|p| p.parse::<auki_sdk::PeerId>().is_ok()),
        "Choose at most 64 valid onboarding host peer IDs");
    Ok(())
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    Save { ssid: String, password: Password, hidden: bool, allowed_peer_ids: BTreeSet<String> },
    Remove {},
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Password { Keep {}, Set { value: String }, Clear {} }
pub fn parse_change(bytes: &[u8]) -> Result<Change> {
    ensure!(bytes.len() <= 16384, "Scene Wi-Fi settings exceed bounds");
    serde_json::from_slice(bytes).map_err(|_| anyhow!("Invalid scene Wi-Fi settings request"))
}

/// Browser-visible configuration never includes the password.
#[derive(Serialize)]
pub struct Summary {
    pub ssid: Option<String>,
    pub has_password: bool,
    pub hidden: bool,
    pub allowed_peer_ids: BTreeSet<String>,
}
impl Settings {
    fn summary(&self) -> Summary {
        Summary {
            ssid: self.wifi.as_ref().map(|w| w.ssid.clone()),
            has_password: self.wifi.as_ref().is_some_and(|w| w.password.is_some()),
            hidden: self.wifi.as_ref().is_some_and(|w| w.hidden),
            allowed_peer_ids: self.allowed_peer_ids.clone(),
        }
    }
    fn disclosure(&self, caller: &str) -> Result<Reply> {
        ensure!(caller.parse::<auki_sdk::PeerId>().is_ok(), "Authenticated peer required");
        ensure!(self.wifi.is_none() || self.allowed_peer_ids.contains(caller),
            "This peer is not permitted to retrieve scene Wi-Fi. Authorize it in Home's scene Wi-Fi settings");
        Ok(Reply { wifi: self.wifi.clone() })
    }
}

pub struct Storage {
    directory: PathBuf,
    settings: Mutex<BTreeMap<String, Settings>>,
}
impl Storage {
    pub fn load(directory: PathBuf, scenes: impl Iterator<Item = String>) -> Result<Self> {
        let mut settings = BTreeMap::new();
        for id in scenes {
            ensure!(crate::local_scenes::valid_id(&id), "Invalid scene ID");
            let path = directory.join(format!("{id}.json"));
            let value: Settings = if path.exists() {
                ensure!(std::fs::metadata(&path)?.len() <= 16384, "Scene Wi-Fi settings exceed bounds");
                // Never echo serde errors: malformed secret values can appear in them.
                serde_json::from_slice(&std::fs::read(path)?).map_err(|_| anyhow!("Invalid scene Wi-Fi settings"))?
            } else { Settings::default() };
            if let Some(wifi) = &value.wifi { wifi.validate()?; }
            validate_peers(&value.allowed_peer_ids)?;
            settings.insert(id, value);
        }
        Ok(Self { directory, settings: Mutex::new(settings) })
    }
    pub fn summary(&self, id: &str) -> Result<Summary> {
        Ok(self.settings.lock().map_err(|_| anyhow!("Scene Wi-Fi unavailable"))?
            .get(id).ok_or_else(|| anyhow!("Unknown scene"))?.summary())
    }
    pub fn change(&self, id: &str, change: Change) -> Result<Summary> {
        let mut settings = self.settings.lock().map_err(|_| anyhow!("Scene Wi-Fi unavailable"))?;
        let previous = settings.get(id).ok_or_else(|| anyhow!("Unknown scene"))?;
        let next = match change {
            Change::Remove {} => Settings::default(),
            Change::Save { ssid, password, hidden, allowed_peer_ids } => {
                validate_peers(&allowed_peer_ids)?;
                let password = match password {
                    Password::Keep {} => {
                        ensure!(previous.wifi.as_ref().is_some_and(|w| w.ssid == ssid && w.password.is_some()),
                            "Enter a password for the new network, or choose Open network");
                        previous.wifi.as_ref().and_then(|w| w.password.clone())
                    },
                    Password::Set { value } => Some(value),
                    Password::Clear {} => None,
                };
                let wifi = Wifi { ssid, password, hidden };
                wifi.validate()?;
                Settings { wifi: Some(wifi), allowed_peer_ids }
            },
        };
        // Serialize writers and persist before acknowledging. Files are owner-only.
        persist(&self.directory, id, &next)?;
        let summary = next.summary();
        settings.insert(id.into(), next);
        Ok(summary)
    }
    fn get_for_onboarding(&self, id: &str, caller: &str) -> Result<Reply> {
        self.settings.lock().map_err(|_| anyhow!("Scene Wi-Fi unavailable"))?
            .get(id).ok_or_else(|| anyhow!("Unknown scene"))?.disclosure(caller)
    }
}

fn persist(directory: &Path, id: &str, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(settings)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, directory.join(format!("{id}.json")))?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temporary); }
    result
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {}
impl ContractType for Request { const DATATYPE: &'static str = "spatial-harness.scene-wifi-request/v1"; }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply { pub wifi: Option<Wifi> }
impl ContractType for Reply { const DATATYPE: &'static str = "spatial-harness.scene-wifi-reply/v1"; }

pub fn mount(runtime: &ComponentRuntime, endpoint: &auki_component_protocol::ComponentProtocolEndpoint,
    id: &str, storage: Arc<Storage>) -> Result<Component> {
    let component = runtime.component(ComponentSpec::new(format!("{COMPONENT}:{id}"))
        .operable(OperableContract { name: OPERATION.into(), instruction: Request::DATATYPE.into(),
            result: Reply::DATATYPE.into(), exposure: Exposure::Cluster }))?;
    let id = id.to_owned();
    let operation = component.operable(OPERATION,
        |ctx| ctx.caller_peer_id.parse::<auki_sdk::PeerId>().is_ok(),
        move |ctx, _: Request| storage.get_for_onboarding(&id, &ctx.caller_peer_id)
            .map_err(|e| InvocationError::Rejected(e.to_string())))?;
    component.expose()?;
    endpoint.export_operable(&operation)?;
    Ok(component)
}

/// Same Catalog/Operable path for every onboarding host; no HTTP peer fallback.
pub async fn retrieve(operations: &crate::mobile_home::HomeOperations, home: &crate::network::HomeConnection) -> Result<Option<Wifi>> {
    let reply: Reply = operations.invoke(&home.scoped_component(COMPONENT), OPERATION, "onboarding-host",
        &uuid::Uuid::new_v4().to_string(), std::time::Duration::from_secs(5), &Request {}).await?;
    if let Some(wifi) = &reply.wifi { wifi.validate()?; }
    Ok(reply.wifi)
}
