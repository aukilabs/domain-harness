mod support;
use anyhow::Result;
use spatial_home::{
    mobile_notes::{PlaceNote, intersection},
    model::{FRAME, Store},
    network::{HomeConnection, ScenePublisher},
    raycast::Voxel,
};
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
        let action = |action| ChangeNote { request_id: uuid::Uuid::new_v4().to_string(), frame: FRAME.into(), action };
        let changed = send_change(&pair.b,&home,&action(NoteAction::Edit { id: request.id.clone(), text: "Edited".into() })).await?;
        assert_eq!(changed.note.unwrap().text,"Edited");
        let deleted = send_change(&pair.b,&home,&action(NoteAction::Delete { id: request.id.clone() })).await?;
        assert!(deleted.deleted);
        send_change(&pair.b,&home,&action(NoteAction::Restore { note: deleted.note.unwrap() })).await?;
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
