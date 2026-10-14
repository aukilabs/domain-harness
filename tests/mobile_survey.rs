mod support;
use spatial_home::mobile_survey::{Log, Sample, WifiRow};
use spatial_home::model::FRAME;

fn sample(id: &str) -> Sample {
    Sample {
        id: id.into(),
        captured_at: "2026-10-14T02:00:00Z".into(),
        scan_age_ms: 400,
        translation: [1.5, 1.2, -2.0],
        wifi: vec![WifiRow {
            ssid: "Auki-Office".into(),
            bssid: "aa:bb:cc:dd:ee:ff".into(),
            rssi_dbm: -47,
            frequency_mhz: 5180,
        }],
        cellular: spatial_home::mobile_survey::Cellular {
            radio: "NR".into(),
            dbm: Some(-89),
            level: Some(3),
        },
        path: None,
    }
}

#[test]
fn survey_queue_holds_a_walk() {
    assert_eq!(spatial_home::mobile::SURVEY_QUEUE_CAPACITY, 512);
}

#[test]
fn overlay_keeps_phone_height() {
    let mut log = Log::default();
    log.accept(sample("a")).unwrap();
    let mut view = serde_json::json!({});
    spatial_home::mobile_survey::append_samples(&mut view, &log.snapshot());
    assert_eq!(view["survey_samples"][0]["translation"][1], 1.2);
    assert_eq!(view["survey_samples"][0]["wifi"][0]["ssid"], "Auki-Office");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sample_reaches_home_over_p2p() -> anyhow::Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let store = std::sync::Arc::new(std::sync::Mutex::new(spatial_home::model::Store::open(
            dir.path().join("home.json"),
        )?));
        let publisher = spatial_home::network::ScenePublisher::mount(&pair.a)?;
        let changed = std::sync::Arc::new(tokio::sync::Notify::new());
        let log = std::sync::Arc::new(std::sync::Mutex::new(Log::default()));
        publisher.enable_connectivity_survey(store.clone(), changed, log)?;
        let home = spatial_home::network::HomeConnection::direct(&pair.a);
        let mut cache = None;
        spatial_home::mobile_survey::send(&pair.b, &home, &sample("walk-1"), &mut cache).await?;
        assert!(cache.is_some());
        spatial_home::mobile_survey::send(&pair.b, &home, &sample("walk-2"), &mut cache).await?;
        let registry = store.lock().unwrap().built.registries["connectivity_survey"].clone();
        assert_eq!(registry["frame"], FRAME);
        assert_eq!(registry["samples"][0]["id"], "walk-1");
        assert_eq!(registry["samples"][1]["id"], "walk-2");
        assert_eq!(registry["samples"][0]["translation"][1], 1.2);
        publisher.close().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    pair.shutdown().await?;
    result
}
