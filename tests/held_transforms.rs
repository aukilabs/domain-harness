#[allow(dead_code)]
mod support;
use anyhow::{Result, ensure};
use spatial_home::{compact_pose::Contract, frame_alignment::HeldTransforms};
use std::time::Duration;

async fn until(mut check: impl FnMut() -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check() { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await?;
    Ok(())
}

struct CuttableConnection { route: String, task: tokio::task::JoinHandle<()> }
impl CuttableConnection {
    async fn start(target: &str) -> Result<Self> {
        let port: u16 = target.strip_prefix("/ip4/127.0.0.1/tcp/").unwrap().parse()?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await?;
        let route = format!("/ip4/127.0.0.1/tcp/{}",listener.local_addr()?.port());
        let task = tokio::spawn(async move {
            let (mut incoming,_) = listener.accept().await.unwrap();
            drop(listener);
            let mut outgoing = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut incoming,&mut outgoing).await;
        });
        Ok(Self {route,task})
    }
    async fn cut(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}
impl Drop for CuttableConnection { fn drop(&mut self) { self.task.abort(); } }

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_held_stream_is_shared_idle_revocable_and_catalog_scoped() -> Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let home = pair.a.peer_id().to_string();
        let owner = pair.b.peer_id().to_string();
        let frame = "local-scene:test-room:epoch-11111111-1111-4111-8111-111111111111";
        let definitions = spatial_home::product_registry::Definitions::default();
        spatial_home::product_registry::frame(&definitions, &spatial_home::conventions::registry(&home, frame)?)?;
        let products = spatial_home::mobile_products::MobileProducts::mount(&pair.b)?;
        let session = uuid::Uuid::new_v4().to_string();
        let mut publisher = spatial_home::mobile_declared::Publisher::new(&products, &session, &home, frame)?;
        let transform = |x| auki_scenegraph::RigidTransform {
            from_frame_id:format!("phone-ar:{session}"), to_frame_id:frame.into(),
            translation:[x,0.,0.], rotation_wxyz:[1.,0.,0.,0.],
        };
        // Publisher retains eight. Bootstrap must handle an explicit initial gap,
        // and cannot invent a transform for captures before the retained tail.
        for i in 1..=10 { publisher.align(i * 10, &transform(i as f64))?; }
        let routes = pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
        let mut row = spatial_home::device_catalogs::fetch(&pair.a, pair.b.peer_id(), &routes).await;
        row["registries"] = spatial_home::product_registry::resolve(&pair.a, pair.b.peer_id(), &row, None, &definitions).await;
        ensure!(row["registries"]["status"] == "ready", "{row}");
        let catalog: auki_components::CatalogSnapshot = serde_json::from_value(row["catalog"].clone())?;
        let entry = catalog.products.iter().find(|p|p.metadata.as_ref().is_some_and(|m|m.schema==spatial_home::compact_pose::METADATA)).unwrap();
        let reference = entry.manifest.reference();
        let contract: Contract = serde_json::from_value(entry.metadata.as_ref().unwrap().value.clone())?;
        let mut connection = CuttableConnection::start(&routes[0]).await?;
        let direct_route = row["catalog_route"].clone();
        row["catalog_route"] = connection.route.clone().into();
        let mut rows = std::collections::BTreeMap::from([(owner.clone(), row)]);
        let (tx, rx) = tokio::sync::watch::channel(rows.clone());
        let cache = HeldTransforms::default();
        let other_reader = cache.clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let exercise = async {
            until(||cache.at(&reference,&contract,9,100).is_ok()).await?;
            assert!(cache.at(&reference,&contract,9,20).is_err());
            assert_eq!(cache.at(&reference,&contract,9,35)?.0.translation[0],3.);
            assert_eq!(cache.at(&reference,&contract,9,99)?.0.translation[0],9.);
            assert!(cache.at(&reference,&contract,1000,100).is_err(), "must catch up to advertised metadata");
            // No new publications or TTL heartbeat; both consumers reuse one stream.
            tokio::time::sleep(Duration::from_millis(2100)).await;
            for _ in 0..100 {
                assert_eq!(cache.at(&reference,&contract,9,1_000_000)?.0.translation[0],10.);
                assert_eq!(other_reader.at(&reference,&contract,9,1_000_000)?.1,100);
            }
            assert_eq!(products.endpoint.active_subscriptions(),1);
            // A failed transport invalidates an otherwise idle, valid held state.
            connection.cut().await;
            until(||cache.at(&reference,&contract,9,100).is_err()).await?;
            // Rebind an advertised working route, preserving identity, not validity.
            rows.get_mut(&owner).unwrap()["catalog_route"] = direct_route;
            tx.send_replace(rows.clone());
            until(||cache.at(&reference,&contract,9,100).is_ok()).await?;
            // Deliberately never refresh the Catalog: these must arrive by push.
            publisher.align(110,&transform(11.))?;
            until(||cache.at(&reference,&contract,10,110).is_ok()).await?;
            assert_eq!(cache.at(&reference,&contract,10,109)?.0.translation[0],10.);
            assert_eq!(cache.at(&reference,&contract,10,110)?.0.translation[0],11.);
            publisher.lose()?; // revocation at 111
            until(||cache.at(&reference,&contract,11,111).is_err_and(|e|e.to_string().contains("revoked"))).await?;
            assert_eq!(cache.at(&reference,&contract,11,110)?.0.translation[0],11.);
            publisher.align(120,&transform(12.))?;
            until(||cache.at(&reference,&contract,12,120).is_ok()).await?;
            assert!(cache.at(&reference,&contract,12,115).is_err(), "do not cross a revocation");
            let mut foreign = reference.clone(); foreign.manifest_hash = "wrong".into();
            assert!(cache.at(&foreign,&contract,0,120).is_err());
            let mut changed = contract.clone(); changed.to_frame.id = "new-origin".into();
            assert!(cache.at(&reference,&changed,0,120).is_err());
            // Withdrawal invalidates cached values and awaits the subscription.
            tx.send_replace(Default::default());
            until(||cache.at(&reference,&contract,0,120).is_err() && products.endpoint.active_subscriptions()==0).await?;
            tx.send_replace(rows);
            until(||cache.at(&reference,&contract,12,120).is_ok()).await?;
            assert_eq!(products.endpoint.active_subscriptions(),1);
            // Explicit closure is not an idle held transform.
            products.endpoint.unexport_product(&reference.product_id);
            until(||cache.at(&reference,&contract,0,120).is_err()).await?;
            Ok::<_,anyhow::Error>(())
        };
        let (_, checked) = tokio::join!(cache.serve(&pair.a,rx,&stop), async {
            let result = exercise.await;
            stop.cancel();
            result
        });
        assert!(cache.at(&reference,&contract,0,120).is_err());
        until(||products.endpoint.active_subscriptions()==0).await?;
        publisher.close()?;
        products.close().await?;
        checked
    }.await;
    result.and(pair.shutdown().await)
}
