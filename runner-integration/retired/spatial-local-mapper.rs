//! Standalone, read-only sensor mapper anchored by its first observed portal.
use anyhow::{Result, anyhow, ensure};
use spatial_home::{
    local_mapper::{LocalMap, SensorEvent},
    network::{HomeConnection, ScenePublisher},
    peer::PeerSettings,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, BufReader};
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "Usage: spatial-local-mapper PEER.json SCENE.json PROFILE.json"
    );
    let path = PathBuf::from(&args[2]);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.try_lock()?;
    let running = PeerSettings::load(std::path::Path::new(&args[1]))?
        .start()
        .await?;
    let work=async {
        let session=uuid::Uuid::new_v4().to_string();
        let frame=format!("galbot-local:{session}:zup");
        let mut publisher=ScenePublisher::mount_in_frame(&running.peer,&frame)?;
        let mut local=LocalMap::new(&publisher.runtime,&running.peer.peer_id().to_string(),&session)?;
        local.export_sources(&publisher.endpoint)?;
        let mut connection=HomeConnection::current(&running.peer)?;connection.frame=frame.clone();
        save(std::path::Path::new(&args[3]),&connection)?;
        // Never relabel old geometry under a new odometry session or portal origin.
        if path.exists() {let archive=path.with_extension(format!("previous-{session}.json"));std::fs::rename(&path,archive)?;}
        let script=std::env::var_os("SPATIAL_LOCAL_CAPTURE").map(PathBuf::from).unwrap_or_else(||"scripts/capture-local.py".into());
        let mut child=tokio::process::Command::new("python3.8").arg(script)
            .env("PYTHONPATH","/data/galbot/lib:/data/galbot/lib/python3.8.10:/data/galbot/lib/python3/site-packages")
            .env("LD_LIBRARY_PATH","/data/galbot/lib").env_remove("AUKI_ACCESS_KEY").env_remove("AUKI_APP_SECRET")
            .stdout(std::process::Stdio::piped()).kill_on_drop(true).spawn()?;
        let mut lines=BufReader::new(child.stdout.take().unwrap()).lines();
        let (send,mut receive)=tokio::sync::mpsc::channel(256);
        let reader=tokio::spawn(async move {
            while let Some(line)=lines.next_line().await? {
                if let Some(raw)=line.strip_prefix("SENSOR ") {
                    ensure!(raw.len()<4*1024*1024,"sensor packet exceeds limit");
                    if send.send(serde_json::from_str::<SensorEvent>(raw)?).await.is_err(){break;}
                }
            }
            Ok::<_,anyhow::Error>(())
        });
        let mut camera_exported=false;
        let mut last_publish=Instant::now();let mut last_odom=Instant::now();
        let mut tick=tokio::time::interval(Duration::from_millis(500));
        #[cfg(unix)]
        let mut terminate=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let run=async {
            loop {
                tokio::select! {
                    _=tokio::signal::ctrl_c()=>break,
                    _=terminate.recv()=>break,
                    _=tick.tick()=>{
                        if last_odom.elapsed()>Duration::from_secs(3) {local.latest_pose=None;local.detail="Odometry unavailable; waiting for measured poses".into();}
                        if !camera_exported {camera_exported=local.export_camera_preview(&publisher.endpoint)?;}
                        local.revision+=1; // Publication contains independently changing robot pose/status.
                        if let Some(scene)=publisher.publish_local(&local)? {
                            save(&path,&scene)?;
                        }
                        save(&path.with_extension("status.json"),&serde_json::json!({"frame":frame,"revision":local.revision,"origin":local.origin,"detail":local.detail,"status":if local.origin.is_some(){"mapping"}else{"waiting_for_odometry"}}))?;
                        last_publish=Instant::now();
                    },
                    value=receive.recv()=>{
                        let event=value.ok_or_else(||anyhow!("sensor capture ended"))?;
                        let is_odom=matches!(&event,SensorEvent::Odometry{..});
                        match local.accept(event) {
                            Ok(())=>{if is_odom {last_odom=Instant::now();}},
                            Err(error)=>{local.detail=format!("Observation rejected: {error}");eprintln!("{}",local.detail);},
                        }
                        if last_publish.elapsed()>Duration::from_secs(5) {tokio::task::yield_now().await;}
                    }
                }
            }
            Ok::<_,anyhow::Error>(())
        }.await;
        let _=child.start_kill();let _=child.wait().await;reader.abort();let _=reader.await;
        let cleanup=publisher.close().await;run.and(cleanup)
    }.await;
    let cleanup = tokio::time::timeout(Duration::from_secs(15), running.shutdown()).await?;
    work.and(cleanup)
}
fn save(path: &std::path::Path, value: &impl serde::Serialize) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
