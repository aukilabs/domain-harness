#[allow(dead_code)]
mod support;
use anyhow::Result;
use spatial_home::{device_catalogs, frame_alignment::HeldTransforms, mobile_odometry, mobile_products::MobileProducts, product_registry};
use std::time::{Duration, Instant};

#[tokio::test(flavor="multi_thread", worker_threads=4)]
async fn native_poses_are_buffered_before_alignment_and_used_when_it_arrives() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let scenes = spatial_home::local_scenes::LocalScenes::load(dir.path())?;
        let frame = scenes.view("humanoid-hub-conference")?["frame"].as_str().unwrap().to_owned();
        let home = pair.a.peer_id().to_string();
        let phone = pair.b.peer_id().to_string();
        let products = MobileProducts::mount(&pair.b)?;
        let session = uuid::Uuid::new_v4().to_string();
        let mut sample = spatial_home::mobile_pose::Sample { session:session.clone(), timestamp_ns:1,
            camera_configuration_id:None, pose:auki_scenegraph::RigidTransform {
                from_frame_id:format!("phone-camera:{session}"),to_frame_id:format!("phone-ar:{session}"),
                translation:[1.,1.5,0.],rotation_wxyz:[1.,0.,0.,0.] } };
        let mut native = mobile_odometry::Publisher::new(&products,&home,&sample)?;
        native.publish(&home,&sample)?;
        let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
        // Authenticate and retain only its verified route (not its Catalog).
        // The SDK known-peer list is intentionally not itself a route source.
        // An unavailable DDS must not gate refresh on this connected route.
        let bootstrap=device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
        assert_eq!(bootstrap["catalog_status"],"ready");
        let entry = products.runtime.catalog().snapshot().products.into_iter()
            .find(|p|p.manifest.product_id==mobile_odometry::product_id(&session)).unwrap();
        let reference = entry.manifest.reference();
        let contract:spatial_home::compact_pose::Contract = serde_json::from_value(entry.metadata.unwrap().value)?;
        let definitions = product_registry::Definitions::default();
        let (tx,rx) = tokio::sync::watch::channel(std::collections::BTreeMap::from([(phone.clone(),
            serde_json::json!({"catalog_status":"loading","catalog_route":bootstrap["catalog_route"]}))]));
        let stop = tokio_util::sync::CancellationToken::new();
        let history = HeldTransforms::default();
        let monitor = device_catalogs::monitor(&pair.a,&definitions,&scenes,&[],tx,&stop);
        let consumer = history.serve(&pair.a,rx.clone(),&stop);
        let check = async {
            let result = async {
                let began = Instant::now();
                tokio::time::timeout(Duration::from_secs(2),async {
                    while products.endpoint.product_active_subscriptions(&reference.product_id)==0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await?;
                // Native capture is consumable without a Home transform. There
                // is no ten-second warmup and no historical replay as liveness.
                sample.timestamp_ns=100;native.publish(&home,&sample)?;
                tokio::time::timeout(Duration::from_secs(1),async {
                    while history.snapshot(&reference,&contract).is_err() {tokio::time::sleep(Duration::from_millis(10)).await;}
                }).await?;
                let before = history.snapshot(&reference,&contract)?;
                assert_eq!(before.latest.timestamp_ns,100);
                assert!(began.elapsed()<Duration::from_secs(3));
                eprintln!("Native pose buffered before calibration in {:?}",began.elapsed());
                assert!(rx.borrow()[&phone]["catalog"]["products"].as_array().unwrap().iter()
                    .all(|p|p["metadata"]["value"]["lifetime"]!="held"||p["metadata"]["schema"]!=spatial_home::compact_pose::METADATA));

                // Calibration is published later, effective at capture 100.
                // It must not restart the native subscription or erase history.
                let mut alignment = spatial_home::mobile_declared::Publisher::new(&products,&session,&home,&frame)?;
                alignment.align(100,&auki_scenegraph::RigidTransform {
                    from_frame_id:contract.to_frame.id.clone(),to_frame_id:frame.clone(),
                    translation:[2.,0.,0.],rotation_wxyz:[1.,0.,0.,0.] })?;
                let began = Instant::now();
                let aligned = tokio::time::timeout(Duration::from_secs(3),async {
                    loop {
                        let rows=rx.borrow().clone();
                        if let Some(row)=rows.get(&phone) {
                            if let Some(p)=row["catalog"]["products"].as_array().and_then(|ps|ps.iter().find(|p|p["manifest"]["product_id"]==spatial_home::mobile_declared::transform_product_id(&session))) {
                                if let Ok(c)=serde_json::from_value::<spatial_home::compact_pose::Contract>(p["metadata"]["value"].clone()) {
                                    let manifest:auki_components::ProductManifest=serde_json::from_value(p["manifest"].clone())?;
                                    if let Ok((pose,_))=history.at(&manifest.reference(),&c,p["metadata"]["source_sequence"].as_u64().unwrap(),100) {break Ok::<_,anyhow::Error>(pose);}
                                }
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await??;
                assert_eq!(aligned.translation,[2.,0.,0.]);
                assert!(began.elapsed()<Duration::from_secs(3));
                eprintln!("Late alignment resolved against retained pose in {:?}",began.elapsed());
                let retained=history.snapshot(&reference,&contract)?;
                assert_eq!(retained.latest.timestamp_ns,100);
                assert_eq!(retained.latest.sequence,before.latest.sequence);
                assert_eq!(products.endpoint.product_active_subscriptions(&reference.product_id),1);
                // Resolve the existing capture, without requiring another pose.
                let (pose,_)=history.at(&reference,&contract,retained.latest.sequence,100)?;
                assert_eq!(spatial_home::local_mapper::compose(&aligned,&pose)?.translation,[3.,1.5,0.]);
                alignment.close()?;
                Ok::<_,anyhow::Error>(())
            }.await;
            stop.cancel();result
        };
        let (_,_,checked)=tokio::join!(monitor,consumer,check);
        assert!(rx.borrow().is_empty());
        tokio::time::timeout(Duration::from_secs(2),async {
            while products.endpoint.product_active_subscriptions(&reference.product_id)!=0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await?;
        native.close()?;products.close().await?;
        checked
    }.await;
    result.and(pair.shutdown().await)
}
