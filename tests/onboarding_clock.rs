#[allow(dead_code)]
mod support;
use anyhow::Result;
use auki_component_protocol::ComponentProtocolClient;
use auki_odometry::Pose;
use spatial_home::{clock_sync, mobile_onboarding as mobile, mobile_products::MobileProducts, onboarding as ob};
use std::time::Duration;

fn pose(x: f64) -> Pose { Pose { translation: [x, 0., 0.], rotation_xyzw: [0., 0., 0., 1.] } }

/// Run the real phone host and joiner over authenticated loopback libp2p.
/// Capture timestamps are deliberately delayed, never replaced by receipt time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn onboarding_uses_registry_clocks_and_rejects_changed_identity() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let host = MobileProducts::mount(&pair.b)?;
    let joiner = MobileProducts::mount(&pair.a)?;
    let result = async {
        let home = spatial_home::network::HomeConnection::direct(&pair.a);
        let routes: Vec<String> = pair.b.listen_addresses().iter().map(ToString::to_string).collect();
        let invitation = ob::Invitation::new("Joiner", home.clone(), ob::Host {
            peer_id: pair.b.peer_id().to_string(), routes: routes.clone(),
        }, None)?;
        let hosted = mobile::Shared::default();
        *hosted.hosting.lock().unwrap() = Some(ob::Hosting {
            session: invitation.session.clone(), alias: invitation.alias.clone(), invitation: invitation.encode()?,
            marker: invitation.marker_payload(), marker_in_camera: pose(0.), scene_peer_id: home.peer_id.clone(),
            scene_frame: home.frame.clone(), joiner: None, stage: None, detail: String::new(),
        });
        let ar_session = uuid::Uuid::new_v4().to_string();
        let (_, placement) = tokio::sync::watch::channel(Some(spatial_home::mobile_declared::Placement {
            session: ar_session.clone(), timestamp_ns: 1,
            session_to_scene: auki_scenegraph::RigidTransform::identity(format!("phone-ar:{ar_session}"), &home.frame),
        }));
        let _display = mobile::Display::mount(&host, &hosted, placement)?;
        let shared = mobile::Shared::default();
        *shared.joining.lock().unwrap() = Some(mobile::Joining {
            session: invitation.session.clone(), alias: invitation.alias.clone(), marker: None, looking: false,
            stage: None, detail: String::new(), sightings: 0, clock: None, alignment: None,
        });
        let (send, mut receive) = tokio::sync::mpsc::channel(4);
        let stop = tokio_util::sync::CancellationToken::new();
        let _guard = stop.clone().drop_guard();
        let host_clock = host.timing.clock_reference()?;
        let join_clock = joiner.timing.clock_reference()?;
        let native_epoch = 100_000_000_000;
        let feed_poses = async {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = tick.tick() => {} }
                let t = host.timing.clock.now_ns();
                hosted.history.lock().unwrap().push(&host_clock, &ar_session, t, pose(t as f64 / 1e10));
            }
        };
        let feed_sightings = async {
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(30)) => {} }
                if !shared.joining.lock().unwrap().as_ref().is_some_and(|j| j.looking) { continue; }
                let timestamp_ns = joiner.timing.clock.now_ns();
                let sighting = mobile::Sighting { session: ar_session.clone(), clock: join_clock.clone(), timestamp_ns,
                    native_timestamp_ns: native_epoch + timestamp_ns,
                    marker_to_session: pose(2. + host.timing.clock.now_ns() as f64 / 1e10), range_m: 0.3 };
                tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(50)) => {} }
                tokio::select! { _ = stop.cancelled() => break, result = send.send(sighting) => if result.is_err() { break; } }
            }
        };
        let work = async {
            let result = tokio::time::timeout(Duration::from_secs(20),
                mobile::join(&pair.a, &home, &invitation, &shared, &mut receive, &joiner)).await;
            stop.cancel();
            result?
        };
        let (_, _, result) = tokio::join!(feed_poses, feed_sightings, work);
        let aligned = result?;
        assert!(aligned.timestamp_ns >= native_epoch, "Only the private app calibration returns native evidence");
        // At 0.1 m/s, incorrectly using arrival time introduces at least 5 mm.
        assert!((aligned.session_to_home.translation[0] + 2.).abs() < 0.002);
        let mapping = shared.joining.lock().unwrap().as_ref().unwrap().clock.clone().unwrap();
        assert_eq!(mapping.from_clock, join_clock);
        assert_eq!(mapping.to_clock, host_clock);

        let client = ComponentProtocolClient::new(pair.a.protocols());
        let registry = auki_protocols::registry::RegistryClient::new(pair.a.protocols());
        let remote = clock_sync::resolve_routes(&client, &registry, pair.b.peer_id(), &routes, &host_clock).await?;
        assert_eq!(remote.contract, host.registered_clock()?.contract);
        assert_eq!(remote.anchor, host.registered_clock()?.anchor);
        let mut wrong = host_clock.clone(); wrong.hash = "0".repeat(64);
        assert!(clock_sync::resolve_routes(&client, &registry, pair.b.peer_id(), &routes, &wrong).await.is_err());
        let next = clock_sync::reference(&auki_time::SessionClock::new(pair.b.peer_id().to_string(), "next", "session").registry_entry());
        assert!(clock_sync::resolve_routes(&client, &registry, pair.b.peer_id(), &routes, &next).await.is_err());
        let component = host.runtime.catalog().snapshot().components.into_iter()
            .find(|c| c.manifest.component_id == ob::COMPONENT).unwrap().manifest.reference();
        let request = ob::ReadClock { session: invitation.session.clone(), clock: host_clock.clone() };
        let reading = client.invoke_exact::<_, ob::ClockReading>(pair.b.peer_id(), routes[0].parse()?, component.clone(),
            "clock", "test", "valid-clock", Some(Duration::from_secs(2)), &request).await?.result;
        assert_eq!(reading.clock, host_clock);
        assert_eq!(reading.session, invitation.session);
        assert!(client.invoke_exact::<_, ob::ClockReading>(pair.b.peer_id(), routes[0].parse()?, component.clone(),
            "clock", "test", "wrong-clock", Some(Duration::from_secs(2)),
            &ob::ReadClock {clock: wrong, ..request.clone()}).await.is_err());
        assert!(client.invoke_exact::<_, ob::ClockReading>(pair.b.peer_id(), routes[0].parse()?, component.clone(),
            "clock", "test", "wrong-session", Some(Duration::from_secs(2)),
            &ob::ReadClock {session: "other".into(), ..request}).await.is_err());
        assert!(client.invoke_exact::<_, ob::MarkerAt>(pair.b.peer_id(), routes[0].parse()?, component,
            "marker_at", "test", "wrong-marker-clock", Some(Duration::from_secs(2)),
            &ob::AskMarker {session: invitation.session, clock: next, timestamp_ns: 1, uncertainty_ns: 0}).await.is_err());
        let definitions = std::mem::take(&mut *host.definitions.lock().unwrap());
        let missing = clock_sync::resolve_routes(&client, &registry, pair.b.peer_id(), &routes, &host_clock).await;
        *host.definitions.lock().unwrap() = definitions;
        assert!(missing.is_err(), "Catalog metadata cannot substitute for Registry definitions");
        drop(_display);
        Ok::<_, anyhow::Error>(())
    }.await;
    let host_cleanup = host.close().await;
    let joiner_cleanup = joiner.close().await;
    let peers_cleanup = pair.shutdown().await;
    result.and(host_cleanup).and(joiner_cleanup).and(peers_cleanup)
}
