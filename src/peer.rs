//! Explicit environment configuration; no dev/production fallback or portal calls.
use anyhow::{Context, Result, ensure};
use auki_sdk::{
    AukiPeer, AukiPeerBootstrap, AukiPeerConfig, AuthClient, AuthEnvironment, AuthSession,
    Credentials, DomainSelection,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Login {
    User,
    App,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerSceneConfig {
    pub name: String,
    pub connection: crate::network::HomeConnection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerSettings {
    pub domain_id: String,
    #[serde(default)]
    pub runner: Option<crate::runner_pose::RunnerConfig>,
    #[serde(default)]
    pub runner_scene: Option<crate::network::HomeConnection>,
    #[serde(default)]
    pub runner_scenes: Vec<RunnerSceneConfig>,
    pub api_url: String,
    pub dds_url: String,
    pub dms_url: String,
    pub login: Login,
    pub identity_file: PathBuf,
    #[serde(default)]
    pub listen: Vec<String>,
    #[serde(default)]
    pub advertise: Vec<String>,
    #[serde(default)]
    pub relay: bool,
    #[serde(default)]
    pub advertise_discovery: bool,
}
impl PeerSettings {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        let mut peers = std::collections::BTreeSet::new();
        if let Some(primary) = &config.runner_scene {
            peers.insert(primary.peer_id.clone());
        }
        ensure!(config.runner_scenes.len() <= 16, "too many runner scenes");
        for runner in &config.runner_scenes {
            ensure!(
                !runner.name.trim().is_empty() && runner.name.len() <= 80,
                "invalid runner name"
            );
            ensure!(
                peers.insert(runner.connection.peer_id.clone()),
                "duplicate runner peer"
            );
        }
        if config.identity_file.is_relative() {
            config.identity_file = path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&config.identity_file);
        }
        Ok(config)
    }
    pub async fn start(&self) -> Result<RunningPeer> {
        self.start_with_discovery(
            self.advertise_discovery
                .then_some(auki_sdk::DdsTrackerMode::DiscoverAndAdvertise),
        )
        .await
    }
    pub async fn start_with_discovery(
        &self,
        mode: Option<auki_sdk::DdsTrackerMode>,
    ) -> Result<RunningPeer> {
        if let Some(runner) = &self.runner {
            runner.validate()?;
        }
        let domain = self.domain_id.parse()?;
        // Validate all non-secret settings before authentication makes a request.
        let environment = AuthEnvironment::new(&self.api_url, &self.dds_url)?;
        let mut config = AukiPeerConfig::new(&self.dms_url)?;
        if !self.relay {
            config = config.direct_only();
        }
        config = config
            .with_listen_addresses(
                self.listen
                    .iter()
                    .map(|s| s.parse())
                    .collect::<Result<Vec<_>, _>>()?,
            )?
            .with_advertised_direct_routes(
                self.advertise
                    .iter()
                    .map(|s| s.parse())
                    .collect::<Result<Vec<_>, _>>()?,
            )?;
        let required = |name| {
            std::env::var(name).with_context(|| format!("set {name} in the process environment"))
        };
        let credentials = match self.login {
            Login::User => {
                Credentials::user_password(required("AUKI_EMAIL")?, required("AUKI_PASSWORD")?)
            }
            Login::App => {
                Credentials::app(required("AUKI_ACCESS_KEY")?, required("AUKI_APP_SECRET")?)
            }
        };
        let mut bootstrap =
            AukiPeerBootstrap::authenticate(AuthClient::new(environment)?, credentials, config)
                .await?;
        if let Some(mode) = mode {
            bootstrap = bootstrap.with_dds_tracker(mode);
        }
        match bootstrap
            .start_persistent_peer(DomainSelection::new(domain), &self.identity_file)
            .await
        {
            Ok(peer) => Ok(RunningPeer {
                peer,
                session: bootstrap.session().clone(),
            }),
            Err(error) => {
                bootstrap.session().close().await;
                Err(error.into())
            }
        }
    }
}
pub struct RunningPeer {
    pub peer: AukiPeer,
    session: AuthSession,
}
impl RunningPeer {
    pub async fn shutdown(self) -> Result<()> {
        let stopped = self.peer.shutdown().await;
        self.session.close().await;
        stopped?;
        Ok(())
    }
}

/// Read a bounded public profile, never credentials or a private identity.
pub fn read_connection(path: &Path) -> Result<crate::network::HomeConnection> {
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= 32 * 1024, "connection profile too large");
    Ok(serde_json::from_slice(&bytes)?)
}
