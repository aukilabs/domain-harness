use serde_json::{json,Value};
use spatial_home::scene_routes::{plan,Query};

#[test]
fn projects_to_floor_and_keeps_exact_endpoints_with_annotator_styling_policy(){
    let r=plan([0.05,1.6,0.05],[2.05,0.8,0.05],&[],0.1).unwrap();
    assert_eq!(r.points,vec![[0.05,0.06,0.05],[2.05,0.06,0.05]]);
    assert!(r.crosses_unobserved_floor);
    let floor=(0..=20).map(|x|[x as f64*0.1+0.05,0.05,0.05]).collect::<Vec<_>>();
    assert!(!plan([0.05,1.,0.05],[2.05,1.,0.05],&floor,0.1).unwrap().crosses_unobserved_floor);
}

#[test]
fn routes_around_a_wall_and_never_cuts_occupied_corners(){
    let wall=(-10..=10).map(|z|[1.05,0.55,z as f64*0.1+0.05]).collect::<Vec<_>>();
    let route=plan([0.05,1.,0.05],[2.05,1.,0.05],&wall,0.1).unwrap();
    assert!(route.points.len()>2);
    assert!(route.points.iter().any(|p|p[2].abs()>1.1));
    assert!(route.points.iter().all(|p|p[1]==0.06));
    // Independent dense samples along the rendered geometry, not just A* cells.
    for p in route.points.windows(2) {for n in 0..=1000 {
        let t=n as f64/1000.;let x=p[0][0]+t*(p[1][0]-p[0][0]);let z=p[0][2]+t*(p[1][2]-p[0][2]);
        assert!(!(x>=1. && x<=1.1 && z>=-1. && z<=1.1),"path crosses the wall at {x}, {z}");
    }}
}

#[test]
fn prefers_observed_floor_and_ignores_overhead_and_floor_surfaces_as_obstacles(){
    let mut floor=Vec::new();
    // A mapped U around an unknown area: prefer it over the unseen shortcut.
    for i in 0..=20 {floor.push([i as f64*0.1+0.05,0.05,0.55]);}
    for j in 0..=5 {for x in [0.05,2.05] {floor.push([x,0.05,j as f64*0.1+0.05]);}}
    floor.push([1.05,2.2,0.05]);
    let r=plan([0.05,1.,0.05],[2.05,1.,0.05],&floor,0.1).unwrap();
    assert!(!r.crosses_unobserved_floor);
    assert!(r.points.iter().any(|p|p[2]>=0.5));
}

#[test]
fn blocked_unreachable_degenerate_and_oversized_routes_fail_without_a_straight_fallback(){
    let a=[0.05,1.,0.05];let b=[2.05,1.,0.05];
    assert!(plan(a,b,&[[0.05,0.55,0.05]],0.1).err().unwrap().to_string().contains("start is blocked"));
    assert!(plan(a,b,&[[2.05,0.55,0.05]],0.1).err().unwrap().to_string().contains("destination is blocked"));
    let wall=(-40..40).map(|z|[1.05,0.55,z as f64*0.1+0.05]).collect::<Vec<_>>();
    assert!(plan(a,b,&wall,0.1).err().unwrap().to_string().contains("No floor route"));
    for (from,to,size) in [(a,a,0.1),(a,[100.,0.,0.],0.1),(a,b,0.),([f64::NAN,0.,0.],b,0.1)] {assert!(plan(from,to,&[],size).is_err());}
    // Two obstacles touching at a corner must not admit a diagonal shortcut.
    let corners=vec![[0.15,0.55,0.05],[0.05,0.55,0.15]];
    let r=plan(a,[0.15,1.,0.15],&corners,0.1).unwrap();
    assert!(r.points.len()>2);assert!(r.points.iter().any(|p|p[0]<0. || p[2]<0.));
}

fn network()->Value {let peers=["phone","robot"].map(|id|json!({"peer_id":id,"alias":if id=="phone"{"Dan"}else{"Bracket 0125"},"catalog":{"products":[{"manifest":{"peer_id":id,"product_id":"pose"},"manifest_hash":"hash","metadata":{"schema":spatial_home::compact_pose::METADATA,"value":{}}}]}}));json!({"peers":peers})}
fn pose(frame:&str,x:f64)->Value {json!({"stream":"live","age_ms":10,"home_pose":{"frame":frame,"tracking":{"status":"valid","pose":{"translation":[x,1.5,0.05]}}}})}
fn view()->Value {json!({"frame":"home","voxel_size":0.1,"voxels":[],"notes":[{"id":"note","text":"cheese","position":[3.05,1.,0.05]}],"semantic_volumes":{"items":[{"id":"room","title":"Kitchen","center":[4.05,1.,0.05]}]},"portals":[{"anchor_id":"portal","payload":"marker","pose_in_map":{"translation":[5.05,0.,0.05]}}]})}

#[tokio::test]
async fn generic_peer_peer_and_mixed_targets_share_the_scene_path_contract()->anyhow::Result<()> {
    let (tx,mut rx)=tokio::sync::mpsc::channel::<spatial_home::sensor_preview::Request>(4);
    let reads=std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=reads.clone();
    let worker=tokio::spawn(async move {while let Some(q)=rx.recv().await {
        count.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        assert_eq!(q.home_frame.as_deref(),Some("home"));assert!(q.debug_pose);assert!(!q.ar);assert!(!q.capture);
        let x=if q.sensor.as_deref()==Some("phone/pose"){0.05}else{2.05};q.reply.send(Ok(pose("home",x).into())).unwrap();
    }});
    let refs=[json!({"kind":"peer","id":"phone"}),json!({"kind":"peer","id":"robot"}),json!({"kind":"note","id":"note"}),json!({"kind":"volume","id":"room"}),json!({"kind":"portal","id":"portal"})];
    for (from,to) in [(0,1),(0,2),(2,0),(2,3),(3,4)] {
        let q:Query=serde_json::from_value(json!({"waypoints":[refs[from],refs[to]]}))?;
        let live=q.live_endpoints(&tx,&network(),"home").await?;let r=q.plan(&view(),&network(),"home",&live)?;
        assert_eq!(r.stroke.color,"#ff5fa8");assert_eq!(r.stroke.width_m,0.05);
        assert_eq!(r.stroke.segments[0][0],[r.waypoints[0].position[0],0.06,r.waypoints[0].position[2]]);
        assert_eq!(*r.stroke.segments[0].last().unwrap(),[r.waypoints[1].position[0],0.06,r.waypoints[1].position[2]]);
        let mut wrong=view();wrong["frame"]=json!("other");assert!(q.plan(&wrong,&network(),"home",&live).is_err());
        if from==0 {assert_eq!(r.waypoints[0].name,"Dan");}if to==1 {assert_eq!(r.waypoints[1].name,"Bracket 0125");}
    }
    let before=reads.load(std::sync::atomic::Ordering::Relaxed);
    let q:Query=serde_json::from_value(json!({"waypoints":[refs[0],refs[2],refs[0],refs[1]]}))?;
    let live=q.live_endpoints(&tx,&network(),"home").await?;
    let r=q.plan(&view(),&network(),"home",&live)?;
    assert_eq!(r.waypoints[0].position,r.waypoints[2].position);
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed)-before,2);
    drop(tx);worker.await?;Ok(())
}

#[tokio::test]
async fn stale_lost_missing_or_unaligned_peers_never_produce_a_path(){
    let q:Query=serde_json::from_value(json!({"waypoints":[{"kind":"peer","id":"phone"},{"kind":"note","id":"note"}]})).unwrap();
    for bad in [json!({"stream":"stale"}),json!({"stream":"live","age_ms":10,"home_pose":{"frame":"home","tracking":{"status":"lost"}}}),pose("other",0.),{
        let mut v=pose("home",0.);v["age_ms"]=json!(2000);v
    }] {
        let (tx,mut rx)=tokio::sync::mpsc::channel::<spatial_home::sensor_preview::Request>(1);
        let worker=tokio::spawn(async move{let q=rx.recv().await.unwrap();q.reply.send(Ok(bad.into())).unwrap();});
        assert!(q.live_endpoints(&tx,&network(),"home").await.is_err());worker.await.unwrap();
    }
    let (tx,_rx)=tokio::sync::mpsc::channel(1);
    assert!(q.live_endpoints(&tx,&json!({"peers":[]}),"home").await.is_err());
}

#[tokio::test]
async fn a_pose_that_ages_out_while_waiting_for_the_second_endpoint_is_rejected(){
    let q:Query=serde_json::from_value(json!({"waypoints":[{"kind":"peer","id":"phone"},{"kind":"peer","id":"robot"}]})).unwrap();
    let (tx,mut rx)=tokio::sync::mpsc::channel::<spatial_home::sensor_preview::Request>(2);
    let worker=tokio::spawn(async move {
        let a=rx.recv().await.unwrap();let b=rx.recv().await.unwrap();
        let mut almost_stale=pose("home",0.05);almost_stale["age_ms"]=json!(1990);
        a.reply.send(Ok(almost_stale.into())).unwrap();tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        b.reply.send(Ok(pose("home",2.05).into())).unwrap();
    });
    assert!(q.live_endpoints(&tx,&network(),"home").await.err().unwrap().to_string().contains("stale"));worker.await.unwrap();
}

#[tokio::test]
async fn invalid_entities_or_schema_are_not_treated_as_coordinates(){
    let (tx,_rx)=tokio::sync::mpsc::channel(1);
    let q:Query=serde_json::from_value(json!({"waypoints":[{"kind":"note","id":"missing"},{"kind":"portal","id":"portal"}]})).unwrap();
    let live=q.live_endpoints(&tx,&network(),"home").await.unwrap();assert!(q.plan(&view(),&network(),"home",&live).is_err());
    assert!(serde_json::from_value::<Query>(json!({"waypoints":[{"kind":"peer","id":"phone","position":[0,0,0]},{"kind":"note","id":"note"}]})).is_err());
}

#[tokio::test]
async fn ordered_waypoints_preserve_every_stop_and_fail_the_entire_path_if_any_leg_fails()->anyhow::Result<()> {
    let q:Query=serde_json::from_value(json!({"waypoints":[{"kind":"note","id":"note"},{"kind":"portal","id":"portal"},{"kind":"volume","id":"room"},{"kind":"note","id":"note"}]}))?;
    let (tx,_rx)=tokio::sync::mpsc::channel(1);
    let live=q.live_endpoints(&tx,&network(),"home").await?;
    let r=q.plan(&view(),&network(),"home",&live)?;
    assert_eq!(r.waypoints.len(),4);
    assert_eq!(r.stroke.segments.len(),1);
    assert_eq!(r.stroke.segments[0],vec![[3.05,0.06,0.05],[5.05,0.06,0.05],[4.05,0.06,0.05],[3.05,0.06,0.05]]);
    let mut blocked=view();blocked["voxels"]=json!([{"center":[4.05,0.55,0.05]}]);
    let error=q.plan(&blocked,&network(),"home",&live).err().unwrap().to_string();
    assert!(error.contains("Leg 2"));assert!(error.contains("destination is blocked"));
    for count in [0,1,65] {
        let bad:Query=serde_json::from_value(json!({"waypoints":vec![json!({"kind":"note","id":"note"});count]}))?;
        assert!(bad.live_endpoints(&tx,&network(),"home").await.is_err());
    }
    Ok(())
}
