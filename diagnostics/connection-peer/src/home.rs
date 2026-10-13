//! Selected Home scene, fetched only through authenticated SDK Products/Registry.
use anyhow::{Context, Result, ensure};
use auki_component_protocol::{
    CatalogResponse, ComponentProtocolClient, ObservationRequest, ObservationSelection,
};
use auki_components::*;
use auki_protocols::registry::RegistryClient;
use auki_registry::{ClockRegistryEntry, FrameRegistryEntry};
use auki_scenegraph::MapSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Scene {
    pub schema: String,
    pub authority_peer_id: String,
    pub publication_clock: ClockRegistryEntry,
    pub portals: MapSnapshot,
}
impl ContractType for Scene {
    const DATATYPE: &'static str = "spatial-harness.mobile-scene/v1";
}
#[derive(Clone)]
pub struct Verified {
    pub portals: MapSnapshot,
    pub frame: FrameRegistryEntry,
    pub identity: String,
}
#[cfg(test)]
pub fn validate(
    scene: &Scene,
    home: &crate::Home,
    output: &OutputManifest,
    frame: &FrameRegistryEntry,
    clock: &ClockRegistryEntry,
) -> Result<Verified> {
    ensure!(scene.schema==Scene::DATATYPE && scene.authority_peer_id==home.peer_id,"Home scene authority mismatch");
    ensure!(scene.publication_clock==*clock,"Home publication clock mismatch");
    validate_portals(&scene.portals,home,output,frame,clock)
}
fn validate_portals(
    portals:&MapSnapshot, home:&crate::Home, output:&OutputManifest,
    frame:&FrameRegistryEntry,clock:&ClockRegistryEntry,
)->Result<Verified>{
    output.validate().map_err(anyhow::Error::msg)?;
    portals.validate()?;
    let map = &portals.scenegraph.map;
    ensure!(
        output.peer_id == home.peer_id
            && output.payload.datatype() == Scene::DATATYPE
            && output.payload.schema() == Scene::DATATYPE,
        "Unexpected Home output"
    );
    ensure!(
        map.frame.id == home.frame && frame.peer_id == home.peer_id && frame.frame_id == home.frame,
        "Home frame/profile mismatch; refresh the connection profile after an origin reset"
    );
    let declared = output
        .spatial_frame
        .as_ref()
        .context("Home output lacks registered frame")?;
    ensure!(
        declared.definition == *frame,
        "Home output/Registry frame mismatch"
    );
    ensure!(map.frame.meters_per_unit == 1., "Home must declare metres");
    let convention = map.frame.convention.context("Home axes are undeclared")?;
    ensure!(
        FrameRegistryEntry::in_convention(&home.peer_id, &home.frame, convention) == *frame,
        "Portal/Registry axes differ"
    );
    ensure!(
        clock.peer_id == home.peer_id
            && output.clock
                == ClockReference {
                    peer_id: clock.peer_id.clone(),
                    id: clock.clock_id.clone(),
                    hash: clock.hash()
                },
        "Home publication clock mismatch"
    );
    // Only interpretation/portal geometry, not notes/presence, changes calibration authority.
    let identity = manifest_hash(
        &serde_json::json!({"frame":declared.reference,"map":map,"anchors":portals.scenegraph.anchors}),
    );
    Ok(Verified {
        portals: portals.clone(),
        frame: frame.clone(),
        identity,
    })
}
pub async fn fetch(
    client: &ComponentProtocolClient,
    registry: &RegistryClient,
    home: &crate::Home,
    route: auki_sdk::Multiaddr,
) -> Result<Verified> {
    let peer = home.peer_id.parse()?;
    let CatalogResponse::Snapshot { snapshot } =
        client.catalog_exact(peer, route.clone(), None).await?
    else {
        anyhow::bail!("Home Catalog unavailable")
    };
    let product = snapshot
        .products
        .iter()
        .find(|p| p.manifest.product_id == home.product_id && p.manifest.peer_id == home.peer_id)
        .context("Selected Home scene Product unavailable")?;
    ensure!(
        product.manifest_hash == product.manifest.hash(),
        "Home Product hash mismatch"
    );
    let output = snapshot
        .components
        .iter()
        .flat_map(|c| c.current_outputs.values())
        .find(|o| o.manifest.reference() == product.manifest.producer)
        .context("Home producer unavailable")?;
    let f = &output
        .manifest
        .spatial_frame
        .as_ref()
        .context("Missing Home frame")?
        .reference;
    ensure!(
        f.peer_id == home.peer_id && output.manifest.clock.peer_id == home.peer_id,
        "Unexpected Registry authority"
    );
    let frame = registry
        .fetch_frame_exact(peer, route.clone(), &f.id, &f.hash)
        .await?;
    let c = &output.manifest.clock;
    let clock = registry
        .fetch_clock_exact(peer, route.clone(), &c.id, &c.hash)
        .await?;
    // Read the portal layer, not Home's mobile payload (which also includes all
    // voxels). The selected scene output pins the exact owner/frame definition;
    // the SDK MapSnapshot declares its frame, convention and physical units.
    let candidates=snapshot.products.iter().filter_map(|p| {
        if p.manifest.peer_id!=home.peer_id || p.manifest_hash!=p.manifest.hash(){return None;}
        let o=snapshot.components.iter().flat_map(|c|c.current_outputs.values()).find(|o|o.manifest.reference()==p.manifest.producer)?;
        (o.manifest.payload.datatype()==MapSnapshot::DATATYPE && o.manifest.payload.schema()==MapSnapshot::DATATYPE
            && o.manifest.peer_id==home.peer_id && o.manifest.spatial_frame_id.as_deref()==Some(home.frame.as_str())
            && o.manifest.clock==output.manifest.clock).then_some((p,&o.manifest))
    }).collect::<Vec<_>>();
    ensure!(candidates.len()==1,"No unambiguous Home portal layer for the selected scene/clock");
    let (portal_product,portal_output)=candidates[0];
    portal_output.validate().map_err(anyhow::Error::msg)?;
    ensure!(portal_output.spatial_frame.as_ref().is_none_or(|f|f.definition==frame),"Portal layer contradicts the selected scene Registry frame");
    let response = client
        .observations_exact::<MapSnapshot>(
            peer,
            route,
            ObservationRequest {
                product: portal_product.manifest.reference(),
                selection: ObservationSelection::LatestExisting,
            },
        )
        .await?;
    ensure!(
        response.product.reference() == portal_product.manifest.reference()
            && response.producer == *portal_output
            && response.gap.is_none()
            && response.end.is_none(),
        "Home Product changed or ended"
    );
    ensure!(
        response.observations.len() == 1,
        "Home has no unambiguous scene snapshot"
    );
    let observation = &response.observations[0];
    ensure!(
        observation.output == portal_output.reference(),
        "Home observation provenance mismatch"
    );
    validate_portals(&observation.payload,home,&output.manifest,&frame,&clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_foreign_authority_changed_frame_and_inconsistent_clock() {
        let fixture = crate::alignment::tests::scene();
        let clock = ClockRegistryEntry {
            peer_id: "home".into(),
            session_id: "session".into(),
            clock_id: "publication".into(),
            body: auki_registry::ClockBody::UtcClock(auki_registry::ClockMeta {
                unit: "nanoseconds".into(),
                monotonic: false,
                epoch: Some("1970-01-01T00:00:00Z".into()),
                scope: auki_registry::Scope::DeviceLocal,
            }),
        };
        let reference = ClockReference {
            peer_id: "home".into(),
            id: clock.clock_id.clone(),
            hash: clock.hash(),
        };
        let runtime = ComponentRuntime::new("home");
        let component = runtime
            .component(ComponentSpec::new("scene").observable(ObservableContract {
                name: "scene".into(),
                datatype: Scene::DATATYPE.into(),
                schema: Scene::DATATYPE.into(),
                access: vec![ObservationAccess::FollowNew],
                exposure: Exposure::Cluster,
            }))
            .unwrap();
        let output = component
            .configured_observable::<Scene>(
                ConfiguredObservableSpec::new(
                    "scene",
                    "scene",
                    reference,
                    PayloadContract::Structured(StructuredPayloadContract {
                        modality: "scene".into(),
                        datatype: Scene::DATATYPE.into(),
                        schema: Scene::DATATYPE.into(),
                        observes: "scene".into(),
                        unit: None,
                    }),
                )
                .in_registered_frame(fixture.frame.clone()),
            )
            .unwrap();
        let home = crate::Home {
            schema: "spatial-harness.home-connection/v1".into(),
            domain_id: "domain".into(),
            peer_id: "home".into(),
            routes: vec![],
            frame: "scene".into(),
            product_id: "scene-product".into(),
        };
        let scene = Scene {
            schema: Scene::DATATYPE.into(),
            authority_peer_id: "home".into(),
            publication_clock: clock.clone(),
            portals: fixture.portals,
        };
        assert!(validate(&scene, &home, &output.manifest(), &fixture.frame, &clock).is_ok());
        let mut bad = scene.clone();
        bad.authority_peer_id = "stranger".into();
        assert!(validate(&bad, &home, &output.manifest(), &fixture.frame, &clock).is_err());
        let mut bad = home.clone();
        bad.frame = "new-origin".into();
        assert!(validate(&scene, &bad, &output.manifest(), &fixture.frame, &clock).is_err());
        let mut bad = clock.clone();
        bad.session_id = "different".into();
        assert!(validate(&scene, &home, &output.manifest(), &fixture.frame, &bad).is_err());
        let wrong = FrameRegistryEntry::ros_optical("home", "scene");
        assert!(validate(&scene, &home, &output.manifest(), &wrong, &clock).is_err());
    }
}
