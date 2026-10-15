#[allow(dead_code)]
mod support;
mod compact_pose { pub use spatial_home::compact_pose::*; }
mod robot_state { pub use spatial_home::robot_state::*; }
#[path = "../diagnostics/connection-peer/src/poses.rs"]
mod poses;
#[allow(dead_code)]
#[path = "../diagnostics/connection-peer/src/body.rs"]
mod body;

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn home_follows_the_diagnostic_body_on_its_pose_base_frame() -> anyhow::Result<()> {
    let pair=support::LocalPair::start().await?;
    let publisher=poses::Publisher::mount(&pair.b)?;
    let mut robot=body::Body::mount(&publisher)?;
    // Catalog metadata, and so the declared body, appears with the first observation.
    robot.publish(serde_json::from_value(serde_json::json!({"kind":"joints","timestamp_ns":1_000,"positions":{"left_shoulder":0.}}))?)?;
    robot.publish(serde_json::from_value(serde_json::json!({"kind":"battery","timestamp_ns":1_000,"battery":{"voltage":19.,"percent":50.}}))?)?;
    let fault=|id:&str|robot_state::Pointing{request_id:id.into(),phase:"fault".into(),arm:Some("arm_left".into()),
        reason:Some("lj0 at 50 C (limit 50 C)".into()),code:Some("hot".into())};
    robot.publish_pointing(1_000,fault("first"))?;
    let routes=pair.b.listen_addresses().iter().map(ToString::to_string).collect::<Vec<_>>();
    let row=spatial_home::device_catalogs::fetch(&pair.a,pair.b.peer_id(),&routes).await;
    let owner=pair.b.peer_id().to_string();
    let (_catalogs_tx,catalogs)=tokio::sync::watch::channel([(owner.clone(),row)].into_iter().collect());
    let states=spatial_home::robot_states::RobotStates::default();
    let stop=tokio_util::sync::CancellationToken::new();
    let serve=states.serve(&pair.a,catalogs,&stop);
    let check=async {
    let result=async {
    // Home follows new state only, so keep publishing until both kinds arrive.
    // Each arm is its own capture, at its own time; Home merges them by joint.
    let mut t=1_000;
    let view=loop {
        t+=2;
        robot.publish(serde_json::from_value(serde_json::json!({"kind":"joints","timestamp_ns":t,"positions":{"left_shoulder":0.25}}))?)?;
        robot.publish(serde_json::from_value(serde_json::json!({"kind":"joints","timestamp_ns":t+1,"positions":{"right_shoulder":-0.25}}))?)?;
        robot.publish(serde_json::from_value(serde_json::json!({"kind":"health","timestamp_ns":t,
            "joints":{"left_shoulder":{"temperature_c":44.5,"current_a":1.2}}}))?)?;
        robot.publish(serde_json::from_value(serde_json::json!({"kind":"health","timestamp_ns":t+1,
            "joints":{"right_shoulder":{"temperature_c":40.0,"current_a":0.4}}}))?)?;
        robot.publish(serde_json::from_value(serde_json::json!({"kind":"battery","timestamp_ns":t,"battery":{"voltage":19.1,"percent":59.0}}))?)?;
        robot.publish_pointing(t,fault("live"))?;
        let view=states.view();
        let robot_view=&view["robots"][0];
        let both=|kind:&str,field:&str|["left_shoulder","right_shoulder"].iter()
            .all(|joint|!robot_view[kind]["state"][field][joint].is_null());
        if both("joints","positions") && both("health","joints")
            && !robot_view["battery"].is_null() && !robot_view["pointing"].is_null() {break view;}
        anyhow::ensure!(t<1_200,"Home did not receive robot state: {view}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let robot_view=&view["robots"][0];
    assert_eq!(view["robots"].as_array().unwrap().len(),1,"joints and health describe one body");
    assert_eq!(robot_view["peer_id"],owner);
    assert_eq!(robot_view["model"],"bracketbot");
    assert_eq!(robot_view["base_frame"],serde_json::to_value(compact_pose::FrameRef::from(&publisher.contract.from_frame))?);
    assert_eq!(robot_view["session_id"],publisher.contract.session_id);
    assert_eq!(robot_view["joints"]["state"]["positions"]["left_shoulder"],0.25);
    assert_eq!(robot_view["joints"]["state"]["positions"]["right_shoulder"],-0.25);
    assert_eq!(robot_view["health"]["state"]["joints"]["left_shoulder"]["temperature_c"],44.5);
    assert_eq!(robot_view["health"]["state"]["joints"]["right_shoulder"]["temperature_c"],40.0);
    assert_eq!(robot_view["battery"]["state"]["percent"],59.0);
    assert_eq!(robot_view["pointing"]["state"]["phase"],"fault");
    assert_eq!(robot_view["pointing"]["state"]["code"],"hot");
    assert_eq!(robot_view["pointing"]["state"]["reason"],"lj0 at 50 C (limit 50 C)");
    assert!(robot_view["joints"]["age_ms"].as_u64().unwrap()<2_000);
    // A stale or repeated timestamp is never published over newer state.
    robot.publish(serde_json::from_value(serde_json::json!({"kind":"joints","timestamp_ns":5,"positions":{"left_shoulder":9.0}}))?)?;
    assert_ne!(states.view()["robots"][0]["joints"]["state"]["positions"]["left_shoulder"],9.0);
    // A pointing phase at or before the last one is refused, never restamped.
    assert!(robot.publish_pointing(t,fault("late")).is_err());
    anyhow::Ok(())
    }.await;
    // Stop Home's follower on failure too, or join! never returns.
    stop.cancel();
    result
    };
    let ((),result)=tokio::join!(serve,check);
    result?;
    robot.close(&publisher)?;
    publisher.endpoint.close().await?;
    publisher.registry.close().await?;
    pair.shutdown().await?;
    Ok(())
}
