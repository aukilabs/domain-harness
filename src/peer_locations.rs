//! Chat reads the same SDK-backed pose consumer as the map viewer. No peer adapters.
use anyhow::{anyhow, ensure, Result};
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

fn product(peer: &Value) -> Result<&Value> {
    let products = peer["catalog"]["products"].as_array().ok_or_else(||anyhow!("Peer Catalog unavailable"))?;
    let mut sampled = products.iter().filter(|p| p["metadata"]["schema"] == crate::compact_pose::METADATA
        && p["metadata"]["value"]["lifetime"].as_str().unwrap_or("sampled") == "sampled");
    let product = sampled.next().ok_or_else(||anyhow!("No sampled pose Product advertised"))?;
    ensure!(sampled.next().is_none(), "Ambiguous sampled pose Products");
    ensure!(product["manifest"]["peer_id"] == peer["peer_id"], "Pose Product owner mismatch");
    Ok(product)
}

fn location(value: &Value, frame: &str, additional_age: Duration) -> Result<Value> {
    ensure!(value["stream"] == "live", "Pose stream is not live");
    let home = &value["home_pose"];
    ensure!(home["frame"] == frame, "No transform into the selected Home frame");
    ensure!(home["tracking"]["status"] == "valid", "Tracking is not valid");
    let age = value["age_ms"].as_u64().ok_or_else(||anyhow!("Pose age unavailable"))?
        .saturating_add(additional_age.as_millis() as u64);
    ensure!(age < 2000, "Pose is stale");
    let position: [f64;3] = serde_json::from_value(home["tracking"]["pose"]["translation"].clone())?;
    ensure!(position.iter().all(|v|v.is_finite()), "Invalid pose position");
    Ok(json!({"position":position,"frame":frame,"received_age_ms":age,
        "capture_timestamp_ns":value["timestamp_ns"],"source":"registry-backed live pose"}))
}

pub async fn read(tx: &mpsc::Sender<crate::sensor_preview::Request>, peer: &Value, frame: &str) -> Result<Value> {
    let p = product(peer)?;
    let id = p["manifest"]["product_id"].as_str().ok_or_else(||anyhow!("Pose Product ID unavailable"))?;
    let hash = p["manifest_hash"].as_str().ok_or_else(||anyhow!("Pose Product hash unavailable"))?;
    let owner = peer["peer_id"].as_str().ok_or_else(||anyhow!("Peer ID unavailable"))?;
    let work = async {
        loop {
            let (reply, receive) = oneshot::channel();
            tx.try_send(crate::sensor_preview::Request {home_frame:Some(frame.into()),debug_pose:true,
                sensor:Some(format!("{owner}/{id}")),expected_manifest:Some(hash.into()),
                scene:false,ar:false,pick:false,follow:false,capture:false,reply})
                .map_err(|_|anyhow!("Pose request queue unavailable"))?;
            match receive.await.map_err(|_|anyhow!("Pose consumer stopped"))? {
                // A cold viewer subscription starts NewOnly. Wait briefly for its first sample.
                Err(e) if e.to_string() == "Waiting for fresh sensor poses" => tokio::time::sleep(Duration::from_millis(100)).await,
                result => return result,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(3),work).await.map_err(|_|anyhow!("Live pose lookup timed out"))?
}

/// Registry axes, never device kind, define front/left/right.
pub fn relative(value:&Value,frame:&str,direction:&str,distance:f64)->Result<[f64;3]> {
    location(value,frame,Duration::ZERO)?;
    ensure!(distance.is_finite() && (0. ..=1000.).contains(&distance),"Invalid peer offset distance");
    let pose:auki_odometry::Pose=serde_json::from_value(value["home_pose"]["tracking"]["pose"].clone())?;
    let source:auki_registry::FrameRegistryEntry=serde_json::from_value(value["resolved"]["contract"]["from_frame"].clone())?;
    let view=crate::view_camera::frame_view(&pose,&source)?;
    let forward:[f64;3]=serde_json::from_value(view["look_at"].clone())?;
    let up:[f64;3]=serde_json::from_value(view["up"].clone())?;
    let f: [f64;3]=std::array::from_fn(|i|forward[i]-pose.translation[i]);
    let right=[f[1]*up[2]-f[2]*up[1],f[2]*up[0]-f[0]*up[2],f[0]*up[1]-f[1]*up[0]];
    let delta=match direction {"in_front"=>f,"behind"=>f.map(|n|-n),"right"=>right,"left"=>right.map(|n|-n),_=>anyhow::bail!("Invalid peer direction")};
    let length=delta[0].hypot(delta[2]);ensure!(length>1e-6,"Peer direction has no ground-plane projection");
    Ok([pose.translation[0]+delta[0]*distance/length,pose.translation[1],pose.translation[2]+delta[2]*distance/length])
}

pub async fn read_peer(tx:&mpsc::Sender<crate::sensor_preview::Request>,network:&Value,peer:&str,frame:&str)->Result<Value> {
    let row=network["peers"].as_array().and_then(|p|p.iter().find(|p|p["peer_id"]==peer)).ok_or_else(||anyhow!("Peer unavailable"))?;
    let value=read(tx,row,frame).await?;location(&value,frame,Duration::ZERO)?;Ok(value)
}

pub async fn enrich(mut scene: Value, network: Value, tx: mpsc::Sender<crate::sensor_preview::Request>) -> Result<Value> {
    let frame = scene["frame"].as_str().ok_or_else(||anyhow!("Scene frame unavailable"))?.to_owned();
    let peers = scene["peers"].as_array_mut().ok_or_else(||anyhow!("Scene peers unavailable"))?;
    for peer in peers.iter_mut() {if let Some(p)=peer.as_object_mut(){p.remove("location");p.insert("location_error".into(),json!("Live pose lookup timed out"));}}
    let jobs = peers.iter().enumerate().map(|(index,peer)| {
        let row = network["peers"].as_array().and_then(|rows|rows.iter().find(|p|p["peer_id"]==peer["peer_id"])).cloned();
        let tx=tx.clone();let frame=frame.clone();
        async move {let result=match row {Some(row)=>read(&tx,&row,&frame).await,None=>Err(anyhow!("Peer unavailable"))};(index,result,Instant::now())}
    }).collect::<Vec<_>>();
    let mut pending=stream::iter(jobs).buffer_unordered(4);
    let deadline=tokio::time::Instant::now()+Duration::from_secs(8);
    let mut results=Vec::new();
    while let Ok(Some(result))=tokio::time::timeout_at(deadline,pending.next()).await {results.push(result);}
    for (index,result,received) in results {
        match result.and_then(|v|location(&v,&frame,received.elapsed())) {
            Ok(value)=>{peers[index]["location"]=value;peers[index].as_object_mut().unwrap().remove("location_error");},
            Err(e)=>peers[index]["location_error"]=json!(e.to_string()),
        }
    }
    Ok(scene)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn relative_offsets_follow_registry_axes_not_peer_names() {
        for (source,expected) in [
            (auki_registry::FrameRegistryEntry::opengl("arbitrary","camera"),[1.,2.,1.]),
            (auki_registry::FrameRegistryEntry::ros_optical("arbitrary","camera"),[1.,2.,5.]),
            (auki_registry::FrameRegistryEntry::ros_body("arbitrary","body"),[3.,2.,3.])
        ] {
            let v=json!({"stream":"live","age_ms":1,"resolved":{"contract":{"from_frame":source}},"home_pose":{"frame":"home","tracking":{"status":"valid","pose":{"translation":[1,2,3],"rotation_xyzw":[0,0,0,1]}}}});
            assert_eq!(relative(&v,"home","in_front",2.).unwrap(),expected);
            assert!(relative(&v,"other","in_front",2.).is_err());
        }
    }
    #[test] fn rejects_stale_lost_and_other_frames() {
        let v=json!({"stream":"live","age_ms":40,"home_pose":{"frame":"home","tracking":{"status":"valid","pose":{"translation":[1,2,3]}}}});
        assert_eq!(location(&v,"home",Duration::ZERO).unwrap()["position"],json!([1.,2.,3.]));
        assert!(location(&v,"other",Duration::ZERO).is_err());
        assert!(location(&v,"home",Duration::from_secs(2)).is_err());
        let mut lost=v.clone();lost["home_pose"]["tracking"]["status"]=json!("lost");assert!(location(&lost,"home",Duration::ZERO).is_err());
        let mut stale=v;stale["stream"]=json!("stale");assert!(location(&stale,"home",Duration::ZERO).is_err());
    }
    #[tokio::test] async fn arbitrary_peer_uses_viewer_request_and_waits_for_cold_subscription() {
        let peer=json!({"peer_id":"any-peer","catalog":{"products":[{"manifest":{"peer_id":"any-peer","product_id":"any-pose"},"manifest_hash":"hash","metadata":{"schema":crate::compact_pose::METADATA,"value":{}}}]}});
        let mut ambiguous=peer.clone();ambiguous["catalog"]["products"].as_array_mut().unwrap().push(peer["catalog"]["products"][0].clone());assert!(product(&ambiguous).is_err());
        let (tx,mut rx)=mpsc::channel::<crate::sensor_preview::Request>(4);
        let worker=tokio::spawn(async move {
            let first=rx.recv().await.unwrap();first.reply.send(Err(anyhow!("Waiting for fresh sensor poses"))).unwrap();
            let q=rx.recv().await.unwrap();assert!(q.debug_pose);assert!(!q.capture);assert_eq!(q.home_frame.as_deref(),Some("home"));assert_eq!(q.expected_manifest.as_deref(),Some("hash"));assert_eq!(q.sensor.as_deref(),Some("any-peer/any-pose"));
            q.reply.send(Ok(json!({"stream":"live","age_ms":10,"home_pose":{"frame":"home","tracking":{"status":"valid","pose":{"translation":[1,2,3]}}}}))).unwrap();
        });
        let result=enrich(json!({"frame":"home","peers":[{"peer_id":"any-peer"}]}),json!({"peers":[peer]}),tx).await.unwrap();
        assert_eq!(result["peers"][0]["location"]["position"],json!([1.,2.,3.]));worker.await.unwrap();
    }
    #[tokio::test] async fn missing_product_removes_old_location_and_reports_reason() {
        let (tx,_rx)=mpsc::channel(4);
        let scene=json!({"frame":"home","peers":[{"peer_id":"peer","location":{"position":[9,9,9]}}]});
        let result=enrich(scene,json!({"peers":[{"peer_id":"peer","catalog":{"products":[]}}]}),tx).await.unwrap();
        assert!(result["peers"][0]["location"].is_null());
        assert_eq!(result["peers"][0]["location_error"],"No sampled pose Product advertised");
    }
}
