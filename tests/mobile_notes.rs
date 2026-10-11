mod support;
use anyhow::Result;
use spatial_home::{
    mobile_notes::{PlaceNote, intersection},
    model::{FRAME, Store},
    network::{HomeConnection, ScenePublisher},
    raycast::Voxel,
};
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_manifest_is_refreshed_before_executing_the_note_once() -> Result<()> {
    use spatial_home::mobile_notes::{
        COMPONENT, ChangeNote, NoteAction, NoteChange, change_contract, send_change_via,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let mut store = Store::open(dir.path().join("home.json"))?;
        spatial_home::notes::initialize(&mut store)?;
        let publisher = ScenePublisher::mount(&pair.a)?;
        publisher.enable_annotations(
            Arc::new(Mutex::new(store)),
            Arc::new(tokio::sync::Notify::new()),
        )?;
        let home = HomeConnection::direct(&pair.a);
        let operations = spatial_home::mobile_home::HomeOperations::new(&pair.b, &home)?;
        operations.warm().await?;
        publisher.close().await?;
        // Same authenticated peer/component, a replaced manifest without legacy place.
        let replacement = ScenePublisher::mount(&pair.a)?;
        let component = replacement.runtime.component(
            auki_components::ComponentSpec::new(COMPONENT).operable(change_contract()),
        )?;
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let operation = component.operable(
            "change",
            |_| true,
            move |_, r: ChangeNote| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(NoteChange {
                    request_id: r.request_id,
                    note: None,
                    deleted: true,
                })
            },
        )?;
        component.expose()?;
        replacement.endpoint.export_operable(&operation)?;
        let request = ChangeNote {
            request_id: uuid::Uuid::new_v4().to_string(),
            frame: FRAME.into(),
            action: NoteAction::Delete {
                id: "isolated-test".into(),
            },
        };
        assert!(send_change_via(&operations, &home, &request).await?.deleted);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(operations.catalog_refresh_count().await, 2);
        replacement.close().await?;
        Ok(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[test]
fn nearest_floor_or_voxel() -> Result<()> {
    let origin = [0., 2., 0.];
    let direction = [0., -1., 0.];
    let floor = intersection(&[], 1., origin, direction)?.unwrap();
    assert!(floor.1);
    assert_eq!(floor.0.position, [0., 0., 0.]);
    let voxel = Voxel {
        center: [0., 1., 0.],
        evidence: 1.,
        linear_rgb: None,
    };
    let hit = intersection(&[voxel], 0.5, origin, direction)?.unwrap();
    assert!(!hit.1);
    assert_eq!(hit.0.position, [0., 1.25, 0.]);
    let below = Voxel {
        center: [0., -1., 0.],
        evidence: 1.,
        linear_rgb: None,
    };
    assert!(intersection(&[below], 0.5, origin, direction)?.unwrap().1);
    assert!(intersection(&[], 1., origin, [0., 1., 0.])?.is_none());
    assert!(intersection(&[], 1., origin, [1., 0., 0.])?.is_none());
    assert!(intersection(&[], 1., origin, [0., 0., 0.]).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_note_operations_are_confirmed_once_each() -> Result<()> {
    use spatial_home::mobile_notes::{
        COMPONENT, ChangeNote, NoteAction, NoteChange, change_contract, send_change_via,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let pair = support::LocalPair::start().await?;
    let result = async {
        let publisher = ScenePublisher::mount(&pair.a)?;
        let component = publisher.runtime.component(
            auki_components::ComponentSpec::new(COMPONENT).operable(change_contract()),
        )?;
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let operation = component.operable(
            "change",
            |_| true,
            move |_, r: ChangeNote| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(NoteChange {
                    request_id: r.request_id,
                    note: None,
                    deleted: true,
                })
            },
        )?;
        component.expose()?;
        publisher.endpoint.export_operable(&operation)?;
        let home = HomeConnection::direct(&pair.a);
        let operations = spatial_home::mobile_home::HomeOperations::new(&pair.b, &home)?;
        operations.warm().await?;
        let lanes = (0..4).map(|_| async {
            for _ in 0..12 {
                let request = ChangeNote {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    frame: FRAME.into(),
                    action: NoteAction::Delete {
                        id: "isolated-test".into(),
                    },
                };
                let reply = send_change_via(&operations, &home, &request).await?;
                assert_eq!(reply.request_id, request.request_id);
            }
            Ok::<(), anyhow::Error>(())
        });
        // Await every lane even if one fails, before closing the isolated endpoint.
        let results = futures::future::join_all(lanes).await;
        publisher.close().await?;
        for result in results {
            result?;
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            48,
            "No operation may be replayed"
        );
        Ok(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_note_persists_over_authenticated_p2p() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("home.json");
        let mut store = Store::open(&path)?;
        spatial_home::notes::initialize(&mut store)?;
        let store = std::sync::Arc::new(std::sync::Mutex::new(store));
        let changed = std::sync::Arc::new(tokio::sync::Notify::new());
        let mut publisher = ScenePublisher::mount(&pair.a)?;
        publisher.enable_annotations(store.clone(), changed.clone())?;
        publisher.publish(&store.lock().unwrap().built)?;
        let home = HomeConnection::direct(&pair.a);
        let request = PlaceNote {
            id: uuid::Uuid::new_v4().to_string(),
            frame: FRAME.into(),
            origin: [0., 2., 0.],
            direction: [0., -1., 0.],
            text: "Phone floor note".into(),
        };
        let note = spatial_home::mobile_notes::send(&pair.b, &home, &request).await?;
        assert_eq!(note.position, [0., 0., 0.]);
        assert!(note.anchor_surface.unwrap().floor);
        tokio::time::timeout(std::time::Duration::from_secs(1), changed.notified()).await?;
        let again = spatial_home::mobile_notes::send(&pair.b, &home, &request).await?;
        assert_eq!(again.id, request.id);
        use spatial_home::mobile_notes::{ChangeNote, NoteAction, send_change};
        let operations = spatial_home::mobile_home::HomeOperations::new(&pair.b, &home)?;
        operations.warm().await?;
        assert_eq!(operations.catalog_refresh_count().await, 1);
        let action = |action| ChangeNote {
            request_id: uuid::Uuid::new_v4().to_string(),
            frame: FRAME.into(),
            action,
        };
        let changed = spatial_home::mobile_notes::send_change_via(
            &operations,
            &home,
            &action(NoteAction::Edit {
                id: request.id.clone(),
                text: "Edited".into(),
            }),
        )
        .await?;
        assert_eq!(changed.note.unwrap().text, "Edited");
        let delete = action(NoteAction::Delete {
            id: request.id.clone(),
        });
        let deleted =
            spatial_home::mobile_notes::send_change_via(&operations, &home, &delete).await?;
        assert!(deleted.deleted);
        let retried =
            spatial_home::mobile_notes::send_change_via(&operations, &home, &delete).await?;
        assert_eq!(retried.note.as_ref().unwrap().text, "Edited");
        assert_eq!(
            operations.catalog_refresh_count().await,
            1,
            "Each note must not refetch the catalog"
        );
        send_change(
            &pair.b,
            &home,
            &action(NoteAction::Restore {
                note: deleted.note.unwrap(),
            }),
        )
        .await?;
        let mut invalid = request.clone();
        invalid.id = uuid::Uuid::new_v4().to_string();
        invalid.frame = "wrong".into();
        assert!(
            spatial_home::mobile_notes::send(&pair.b, &home, &invalid)
                .await
                .is_err()
        );
        let mut restored = Store::open(path)?;
        spatial_home::notes::initialize(&mut restored)?;
        assert_eq!(
            spatial_home::notes::from_registries(&restored.built.registries, FRAME)?
                .items
                .len(),
            1
        );
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_uses_good_route_despite_failed_first_route_and_reports_missing_component()
-> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let publisher = ScenePublisher::mount(&pair.a)?;
        let mut home = HomeConnection::direct(&pair.a);
        home.routes.insert(0, "/ip4/127.0.0.1/tcp/1".into());
        let operations = spatial_home::mobile_home::HomeOperations::new(&pair.b, &home)?;
        tokio::time::timeout(std::time::Duration::from_secs(5), operations.warm()).await??;
        let error = operations
            .invoke::<_, bool>(
                "not-mounted",
                "change",
                "test",
                "test-id",
                std::time::Duration::from_secs(1),
                &true,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("component is not advertised"));
        assert!(!error.to_string().contains("Home is unavailable"));
        assert_eq!(operations.catalog_refresh_count().await, 1);
        publisher.close().await?;
        Ok(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}
