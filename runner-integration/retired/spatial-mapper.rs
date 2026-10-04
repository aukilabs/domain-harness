//! An independent mapper process on the robot. It consumes observations, never
//! Home's accumulated voxel map, and publishes its locally computed scene.
use anyhow::{Result, bail, ensure};
use auki_component_protocol::RemoteObservationEvent;
use serde::{Deserialize, Serialize};
use spatial_home::{
    model::Store,
    network::{HomeConnection, ScenePublisher},
    peer::PeerSettings,
};
use std::{path::PathBuf, time::Duration};
#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    home_peer: String,
    last_revision: Option<usize>,
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "Usage: spatial-mapper PEER.json HOME-CONNECTION.json MAP.json PROFILE.json"
    );
    let settings = PeerSettings::load(std::path::Path::new(&args[1]))?;
    let home = spatial_home::peer::read_connection(std::path::Path::new(&args[2]))?;
    let path = PathBuf::from(&args[3]);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.try_lock()?;
    let cursor_path = path.with_extension("cursor.json");
    let mut cursor: Cursor = if cursor_path.exists() {
        serde_json::from_slice(&std::fs::read(&cursor_path)?)?
    } else {
        Cursor {
            home_peer: home.peer_id.clone(),
            ..Default::default()
        }
    };
    ensure!(cursor.home_peer == home.peer_id, "input authority changed");
    let running = settings.start().await?;
    let work=async {
        ensure!(home.frame==spatial_home::model::FRAME,"expected Home OpenGL source");
        let reference=loop {
            let attempt=async {
                let mut subscription=spatial_home::network::subscribe_scene(&running.peer,&home).await?;
                let received=tokio::time::timeout(Duration::from_secs(20),subscription.next()).await;
                subscription.close().await?;
                let Some(RemoteObservationEvent::Observation(o))=received?? else {bail!("Home portal snapshot unavailable")};
                o.payload.validate(&home.peer_id)?;
                ensure!(o.payload.frame==spatial_home::model::FRAME,"Home portal convention changed");
                Ok::<_,anyhow::Error>(o.payload.clone())
            };
            match tokio::select! {r=attempt=>r,_=tokio::signal::ctrl_c()=>return Ok(())} {
                Ok(reference)=>break reference,
                Err(error)=>{eprintln!("Fetching Home portals: {error}");tokio::select! {_=tokio::time::sleep(Duration::from_secs(5))=>{},_=tokio::signal::ctrl_c()=>return Ok(())};}
            }
        };
        let mut store=Store::open_mapper(&path,&reference.portals,&running.peer.peer_id().to_string())?;
        ensure!(store.built.view.revision==0||cursor.last_revision.is_some(),"nonempty mapper checkpoint is missing its input cursor");
        store.set_map_owner(&running.peer.peer_id().to_string())?;
        let mut publisher=ScenePublisher::mount(&running.peer)?;
        let rgbd_task = if let Some(script)=std::env::var_os("SPATIAL_RGBD_CAPTURE") {
            let runner=settings.runner.as_ref().ok_or_else(||anyhow::anyhow!("RGB-D needs a configured physical runner"))?;
            let clock=reference.voxels.definition.sources.iter().find(|s|s.sensor_product.peer_id==runner.peer_id)
                .ok_or_else(||anyhow::anyhow!("Home has not established the Galbot capture clock"))?.observation_clock.clone();
            let rgbd=spatial_home::rgbd::Publisher::mount(&publisher.runtime,&publisher.endpoint,&running.peer.peer_id().to_string(),clock)?;
            Some(tokio::spawn(async move {if let Err(error)=rgbd.run(script.into()).await {eprintln!("RGB-D stopped: {error}");}}))
        } else {None};
        let mut connection=HomeConnection::current(&running.peer)?;
        connection.frame=spatial_home::conventions::RUNNER_FRAME.into();
        save(std::path::Path::new(&args[4]),&connection)?;
        publisher.publish(&store.built)?;
        println!("Galbot mapper peer: {}",running.peer.peer_id());
        let run=async {
            loop {
                let connected=tokio::select! {
                    result=spatial_home::shared_inputs::subscribe(&running.peer,&home)=>result,
                    _=tokio::signal::ctrl_c()=>return Ok::<_,anyhow::Error>(()),
                };
                let mut inputs=match connected {
                    Ok(inputs)=>inputs,
                    Err(error)=>{
                        eprintln!("Input connection: {error}");
                        save(&path.with_extension("status.json"),&serde_json::json!({"status":"waiting","detail":error.to_string()}))?;
                        tokio::select! {_=tokio::time::sleep(Duration::from_secs(5))=>{},_=tokio::signal::ctrl_c()=>return Ok(())};
                        continue;
                    }
                };
                let receive=async {
                    loop {
                        match inputs.next().await? {
                            Some(RemoteObservationEvent::Observation(o))=>{
                                let input=&o.payload;
                                if cursor.last_revision.is_some_and(|r|input.revision<=r){continue;}
                                ensure!(cursor.last_revision.is_none_or(|r|input.revision==r+1),"missing accepted scans: retained history no longer covers mapper cursor");
                                if let Some(runner)=&settings.runner {ensure!(input.scan.source.sensor_product.peer_id==runner.peer_id || (input.scan.source.sensor_product.peer_id==running.peer.peer_id().to_string() && input.scan.source.sensor_product.product_id==spatial_home::rgbd::PRODUCT),"unexpected sensor peer");}
                                let applied=store.integrate_prepared(input.scan.clone())?;
                                cursor.last_revision=Some(input.revision);save(&cursor_path,&cursor)?;
                                if applied {publisher.publish(&store.built)?;}
                                let status=serde_json::json!({"peer_id":running.peer.peer_id().to_string(),"home_input_revision":input.revision,"revision":store.built.view.revision,"occupied_voxels":store.built.view.voxels.len(),"status":"mapping"});
                                save(&path.with_extension("status.json"),&status)?;
                            },
                            Some(RemoteObservationEvent::Gap(_))=>{}, // Payload revisions enforce complete continuation.
                            _=>bail!("Home input stream ended"),
                        }
                    }
                };
                let result=tokio::select! {result=receive=>result,_=tokio::signal::ctrl_c()=>Ok(())};
                let cleanup=inputs.close().await;
                match result {
                    Ok(())=>{cleanup?;return Ok::<_,anyhow::Error>(());},
                    Err(error)=>{
                        let _=cleanup;
                        eprintln!("Input connection: {error}");
                        save(&path.with_extension("status.json"),&serde_json::json!({"status":"waiting","detail":error.to_string()}))?;
                        tokio::select! {_=tokio::time::sleep(Duration::from_secs(5))=>{},_=tokio::signal::ctrl_c()=>return Ok(())};
                    },
                }
            }
        };
        let result=run.await;
        if let Some(task)=rgbd_task {task.abort(); let _=task.await;}
        let cleanup=publisher.close().await;
        result.and(cleanup)
    }.await;
    let cleanup = tokio::time::timeout(Duration::from_secs(15), running.shutdown()).await?;
    work.and(cleanup)
}
fn save(path: &std::path::Path, value: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
