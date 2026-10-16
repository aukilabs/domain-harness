#[allow(dead_code)]
mod support;
use anyhow::Result;
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, ComponentProtocolEndpoint};
use auki_components::ComponentRuntime;
use spatial_home::{local_scenes::LocalScenes, scene_wifi::{self, Change, Password}};
use std::{collections::BTreeSet, sync::Arc};

const SCENE: &str = "humanoid-hub-conference";
const FIXTURE_PASSWORD: &str = "fixture-only-password";
fn save(peers: BTreeSet<String>) -> Change {
    Change::Save { ssid: "Fixture network".into(), password: Password::Set { value: FIXTURE_PASSWORD.into() },
        hidden: true, allowed_peer_ids: peers }
}

#[tokio::test]
async fn cancelled_or_superseded_onboarding_worker_cannot_start() -> Result<()> {
    let client = spatial_home::mobile::MobileClient::default();
    let display = || spatial_home::onboarding::Display { model: "fixture".into(), center_points: [0., 0.], side_points: 1. };
    let old = client.prepare_onboarding();
    let current = client.prepare_onboarding();
    assert_ne!(old, current);
    assert_eq!(client.host_onboarding(&old, "Old attempt", display()).await.unwrap_err().to_string(), "Onboarding cancelled");
    client.stop_onboarding();
    assert_eq!(client.host_onboarding(&current, "Cancelled attempt", display()).await.unwrap_err().to_string(), "Onboarding cancelled");
    let replacement = spatial_home::mobile::MobileClient::default();
    let new_client_attempt = replacement.prepare_onboarding();
    assert_ne!(current, new_client_attempt);
    assert_eq!(replacement.host_onboarding(&current, "Wrong client", display()).await.unwrap_err().to_string(), "Onboarding cancelled");
    assert!(client.snapshot().onboarding.is_none());
    assert!(replacement.snapshot().onboarding.is_none());
    Ok(())
}

#[test]
fn scene_wifi_survives_reset_and_is_not_in_spatial_exports() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = LocalScenes::load(dir.path())?;
    let granted = auki_sdk::Identity::generate().peer_id().to_string();
    let summary = scenes.wifi.change(SCENE, save(BTreeSet::from([granted.clone()])))?;
    assert!(summary.has_password && summary.hidden);
    assert!(!serde_json::to_string(&summary)?.contains(FIXTURE_PASSWORD));
    let before = scenes.snapshot(SCENE)?;
    assert!(!serde_json::to_string(before)?.contains("Fixture network"));
    assert!(!serde_json::to_string(&scenes.rows())?.contains("Fixture network"));
    let path = dir.path().join(".settings/wifi").join(format!("{SCENE}.json"));
    let saved = std::fs::read(&path)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap())?.permissions().mode() & 0o777, 0o700);
    }
    spatial_home::origin_reset::stage(dir.path(), before)?;
    let after = LocalScenes::load(dir.path())?;
    assert_ne!(before.scenegraph.map.frame.id, after.snapshot(SCENE)?.scenegraph.map.frame.id);
    assert_eq!(std::fs::read(path)?, saved);
    assert_eq!(after.wifi.summary(SCENE)?.allowed_peer_ids, BTreeSet::from([granted]));
    assert_eq!(after.wifi.summary(SCENE)?.ssid.as_deref(), Some("Fixture network"));
    assert!(after.wifi.summary("nacs")?.ssid.is_none());
    Ok(())
}

#[test]
fn edit_password_explicitly_and_reject_unknown_scenes_and_invalid_values() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = LocalScenes::load(dir.path())?;
    assert!(scenes.wifi.change("../../outside", save(Default::default())).is_err());
    assert!(scenes.wifi.change(SCENE, save(BTreeSet::from(["not a peer".into()]))).is_err());
    scenes.wifi.change(SCENE, save(Default::default()))?;
    let keep = |ssid: &str| Change::Save { ssid: ssid.into(), password: Password::Keep {}, hidden: false, allowed_peer_ids: Default::default() };
    assert!(scenes.wifi.change(SCENE, keep("Different network")).is_err());
    assert!(scenes.wifi.change(SCENE, keep("Fixture network"))?.has_password);
    assert!(!scenes.wifi.change(SCENE, Change::Save { ssid: "Open network".into(), password: Password::Clear {}, hidden: false, allowed_peer_ids: Default::default() })?.has_password);
    assert!(scenes.wifi.change(SCENE, Change::Remove {})?.ssid.is_none());
    assert!(LocalScenes::load(dir.path())?.wifi.summary(SCENE)?.ssid.is_none());
    for bytes in [br#"{"action":"save","password":"private-value"}"#.as_slice(), br#"{"action":"remove","password":"private-value"}"#] {
        let error = scene_wifi::parse_change(bytes).err().unwrap().to_string();
        assert!(!error.contains("private-value"));
    }
    Ok(())
}

#[test]
fn deleting_a_scene_retires_its_credentials_and_grants() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let scenes = LocalScenes::load(dir.path())?;
    scenes.wifi.change(SCENE, save(Default::default()))?;
    std::fs::write(dir.path().join(".settings/catalog.json"), serde_json::to_vec(&serde_json::json!({"deleted":[SCENE]}))?)?;
    let remaining = LocalScenes::load(dir.path())?;
    assert!(remaining.wifi.summary(SCENE).is_err());
    assert!(!dir.path().join(format!(".settings/wifi/{SCENE}.json")).exists());
    assert!(dir.path().join(format!(".deleted/.settings/wifi/{SCENE}.json")).exists());
    Ok(())
}

/// Real SDK Catalog and authenticated loopback libp2p, no live Home or Domain.
#[tokio::test(flavor="multi_thread", worker_threads=4)]
async fn generic_operable_authorizes_exact_peers_and_delivers_wifi_for_invitation() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = Arc::new(LocalScenes::load(dir.path())?);
        scenes.wifi.change(SCENE, save(Default::default()))?;
        let runtime = ComponentRuntime::new(pair.b.peer_id().to_string());
        let endpoint = ComponentProtocolEndpoint::mount(pair.b.protocols(), runtime.clone())?;
        let _component = scene_wifi::mount(&runtime, &endpoint, SCENE, scenes.wifi.clone())?;
        let test = async {
            let home = scenes.connection(SCENE, spatial_home::network::HomeConnection::direct(&pair.b))?;
            let client = ComponentProtocolClient::new(pair.a.protocols());
            let CatalogResponse::Snapshot { snapshot } = client.catalog_exact(pair.b.peer_id(), home.routes[0].parse()?, None).await? else { panic!("Expected Catalog"); };
            assert!(snapshot.products.is_empty()); // No password-bearing Product.
            assert!(!serde_json::to_string(&snapshot)?.contains(FIXTURE_PASSWORD));
            assert!(!serde_json::to_string(&snapshot)?.contains("Fixture network"));
            let operations = spatial_home::mobile_home::HomeOperations::new(&pair.a, &home)?;
            assert!(scene_wifi::retrieve(&operations, &home).await.is_err()); // Authentication is not permission.
            let other = auki_sdk::Identity::generate().peer_id().to_string();
            scenes.wifi.change(SCENE, save(BTreeSet::from([other])))?;
            assert!(scene_wifi::retrieve(&operations, &home).await.is_err());
            scenes.wifi.change(SCENE, save(BTreeSet::from([pair.a.peer_id().to_string()])))?;
            let wifi = scene_wifi::retrieve(&operations, &home).await?.unwrap();
            assert_eq!(wifi.password.as_deref(), Some(FIXTURE_PASSWORD));
            assert!(wifi.hidden);
            assert!(!format!("{wifi:?}").contains(FIXTURE_PASSWORD));
            let invitation = spatial_home::onboarding::Invitation::new("New peer", home.clone(),
                spatial_home::onboarding::Host { peer_id: pair.a.peer_id().to_string(), routes: pair.a.listen_addresses().iter().map(ToString::to_string).collect() }, Some(wifi))?;
            let decoded = spatial_home::onboarding::Invitation::parse(&invitation.encode()?)?;
            assert_eq!(decoded.wifi.unwrap().ssid, "Fixture network");
            assert_eq!(decoded.home.frame, home.frame);
            // Revocation is immediate even with a cached Catalog. No cached password fallback.
            scenes.wifi.change(SCENE, save(Default::default()))?;
            assert!(scene_wifi::retrieve(&operations, &home).await.is_err());
            scenes.wifi.change(SCENE, Change::Remove {})?;
            assert!(scene_wifi::retrieve(&operations, &home).await?.is_none());
            Ok::<_, anyhow::Error>(())
        }.await;
        endpoint.close().await?;
        test
    }.await;
    result.and(pair.shutdown().await)
}
