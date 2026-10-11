mod support;
use anyhow::Result;
use spatial_home::{
    mobile_local::*,
    network::{HomeConnection, ScenePublisher},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_phone_product_reaches_separate_home_view() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let home = ScenePublisher::mount(&pair.a)?;
        mount_receiver(&home.runtime, &home.endpoint)?;
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let phone = Publisher::new(&products)?;
        let session = uuid::Uuid::new_v4().to_string();
        let e = phone.publish(Snapshot {
            session: session.clone(),
            timestamp_ns: 1,
            cells: vec![[-1, 2, 3]],
        })?;
        send(&pair.b, &HomeConnection::direct(&pair.a), &e).await?;
        let view = render(&pair.b.peer_id().to_string())?;
        assert!(view["frame"].as_str().unwrap().contains(&session));
        assert_ne!(view["frame"], spatial_home::model::FRAME);
        assert_eq!(
            view["voxels"][0]["center"],
            serde_json::json!([-0.05, 0.25, 0.35000000000000003])
        );
        let mut spoofed = e.clone();
        spoofed.source.peer_id = pair.a.peer_id().to_string();
        assert!(
            send(&pair.b, &HomeConnection::direct(&pair.a), &spoofed)
                .await
                .is_err()
        );
        let reset = phone.publish(Snapshot {
            session: uuid::Uuid::new_v4().to_string(),
            timestamp_ns: 2,
            cells: vec![],
        })?;
        send(&pair.b, &HomeConnection::direct(&pair.a), &reset).await?;
        assert!(
            render(&pair.b.peer_id().to_string())?["voxels"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        phone.close().await?;
        products.close().await?;
        home.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = pair.shutdown().await;
    result.and(cleanup)
}

#[test]
fn local_map_rejects_invalid_inputs() {
    let mut sample = Snapshot {
        session: uuid::Uuid::new_v4().to_string(),
        timestamp_ns: 1,
        cells: vec![[0, 0, 0]; 10001],
    };
    assert!(sample.validate().is_err());
    sample.cells = vec![[i32::MIN, 0, 0]];
    assert!(sample.validate().is_err());
    sample.cells.clear();
    sample.session = "invalid".into();
    assert!(sample.validate().is_err());
}
