use spatial_home::{scene_navigation::*, scene_paths::{Paths, Storage, Stroke}};
use std::sync::{Arc, Mutex};

/// A wall across x = 1 from z = -2 to z = 2 with a doorway at z = 1..1.8, over seen floor.
fn walled() -> Vec<[f64; 3]> {
    let mut voxels = Vec::new();
    for z in -20..20 {
        if (10..18).contains(&z) { continue; }
        for y in [0.35, 0.95, 1.55] { voxels.push([1.05, y, z as f64 * 0.1 + 0.05]); }
    }
    for x in -10..30 { for z in -20..20 { voxels.push([x as f64 * 0.1 + 0.05, 0.05, z as f64 * 0.1 + 0.05]); } }
    voxels
}
fn person() -> Agent { Agent::default() }

#[test]
fn a_route_goes_round_what_the_map_shows_and_through_the_doorway() -> anyhow::Result<()> {
    let (status, route) = plan([0., 1.4, 0.], [2., 1.2, 0.], &walled(), 0.1, person())?;
    assert_eq!(status, Status::Found);
    assert_eq!((route[0], *route.last().unwrap()), ([0., LIFT_M, 0.], [2., LIFT_M, 0.]));
    assert!(length(&route) > 2.8 && length(&route) < 4., "through the doorway, not round the end of the wall");
    // Every step of the way is clear of the wall.
    for pair in route.windows(2) {
        for step in 0..=50 {
            let t = step as f64 / 50.;
            let (x, z) = (pair[0][0] + (pair[1][0] - pair[0][0]) * t, pair[0][2] + (pair[1][2] - pair[0][2]) * t);
            assert!(!((1.0..1.1).contains(&x) && !(1.0..1.8).contains(&z)), "through the wall at {x},{z}");
        }
    }
    Ok(())
}
#[test]
fn nothing_known_in_the_way_is_direct_and_a_closed_room_is_unreachable() -> anyhow::Result<()> {
    assert_eq!(plan([0., 1., 0.], [1., 1., 1.], &[], 0.1, person())?, (Status::Direct, vec![[0., LIFT_M, 0.], [1., LIFT_M, 1.]]));
    assert_eq!(plan([0., 0., 0.], [0.5, 0., 1.], &walled(), 0.1, person())?.0, Status::Direct);
    // A wall right round the goal, wider than the search goes.
    let mut closed = Vec::new();
    for i in -15..=15 { for (x, z) in [(i, -15), (i, 15), (-15, i), (15, i)] {
        closed.push([5.05 + x as f64 * 0.1, 1., z as f64 * 0.1 + 0.05]);
    } }
    assert_eq!(plan([0., 0., 0.], [5., 0., 0.], &closed, 0.1, person())?, (Status::Unreachable, vec![]));
    assert!(plan([0., 0., 0.], [900., 0., 0.], &walled(), 0.1, person()).is_err(), "too far to plan is refused, not guessed");
    Ok(())
}
#[test]
fn the_agent_sets_what_is_in_the_way() -> anyhow::Result<()> {
    // Something at head height stops a person and not a low robot.
    let beam: Vec<[f64; 3]> = (-20..20).map(|z| [1.05, 1.55, z as f64 * 0.1 + 0.05]).collect();
    assert_eq!(plan([0., 0., 0.], [2., 0., 0.], &beam, 0.1, person())?.0, Status::Found);
    assert_eq!(plan([0., 0., 0.], [2., 0., 0.], &beam, 0.1, Agent { radius_m: 0.25, height_m: 0.6 })?.0, Status::Direct);
    // A wide agent does not fit the doorway a person walks through.
    let wide = plan([0., 0., 0.], [2., 0., 0.], &walled(), 0.1, Agent { radius_m: 0.6, height_m: 1.9 })?;
    assert!(wide.0 == Status::Found && length(&wide.1) > 4.5, "it goes round the end of the wall");
    Ok(())
}

struct Room { paths: Storage, notes: Mutex<Vec<(String, String, [f64; 3])>>, _dir: tempfile::TempDir }
impl Room {
    fn new() -> anyhow::Result<Arc<Self>> {
        let dir = tempfile::tempdir()?;
        Ok(Arc::new(Self { paths: Storage::new(dir.path().into()), _dir: dir,
            notes: Mutex::new(vec![("door".into(), " The door ".into(), [0., 1.4, 0.]), ("printer".into(), "The printer".into(), [2., 1.2, 0.])]) }))
    }
    fn saved(&self) -> Vec<Stroke> { self.paths.get("room", "frame").unwrap().items }
}
struct Seen(Arc<Room>);
impl World for Seen {
    fn voxels(&self) -> anyhow::Result<(Vec<[f64; 3]>, f64)> { Ok((walled(), 0.1)) }
    fn note(&self, id: &str) -> anyhow::Result<Option<(String, [f64; 3])>> {
        Ok(self.0.notes.lock().unwrap().iter().find(|n| n.0 == id).map(|n| (n.1.clone(), n.2)))
    }
    fn store(&self, id: &str, stroke: Option<Stroke>) -> anyhow::Result<Paths> { self.0.paths.change("room", "frame", id, stroke) }
    fn now(&self) -> u64 { 42 }
}
fn clock() -> auki_components::ClockReference {
    spatial_home::model::clock_ref(&spatial_home::model::clock_entry("home", &uuid::Uuid::new_v4().to_string()))
}
fn request(from: Place, to: Place) -> RouteRequest { RouteRequest { frame: "frame".into(), from, to, agent: None, share: None } }

#[test]
fn a_route_is_a_saved_path_and_each_caller_has_one() -> anyhow::Result<()> {
    let room = Room::new()?;
    let home = Navigator::new("frame", clock(), Seen(room.clone()));
    let first = home.plan("phone-a", &request(Place::Point([0., 1.4, 0.]), Place::Note("printer".into())))?;
    let path = first.path.clone().unwrap();
    assert_eq!((first.status, first.map_timestamp_ns, first.frame.as_str()), (Status::Found, 42, "frame"));
    assert_eq!((path.title.as_str(), path.color.as_str(), path.segments.len()), ("Way to The printer", COLOR, 1));
    assert_eq!((path.segments[0][0], *path.segments[0].last().unwrap()), ([0., LIFT_M, 0.], [2., LIFT_M, 0.]));
    assert!(path.segments[0].windows(2).all(|w| ((w[0][0] - w[1][0]).powi(2) + (w[0][2] - w[1][2]).powi(2)).sqrt() <= 0.3001));
    assert_eq!(room.saved(), vec![path.clone()]);
    // The same caller asking again replaces its route; another caller's is its own.
    let between = home.plan("phone-a", &request(Place::Note("door".into()), Place::Note("printer".into())))?.path.unwrap();
    assert_eq!(between.title, "Way from The door to The printer");
    let other = home.plan("phone-b", &request(Place::Point([0., 0., 0.]), Place::Point([0.5, 0., 1.])))?;
    assert_eq!((other.status, other.path.as_ref().unwrap().title.as_str()), (Status::Direct, "Route"));
    assert_eq!(room.saved(), vec![between.clone(), other.path.clone().unwrap()]);
    // A note that moves takes the next route with it.
    room.notes.lock().unwrap()[1].2 = [2., 1.2, -1.];
    let moved = home.plan("phone-a", &request(Place::Note("door".into()), Place::Note("printer".into())))?.path.unwrap();
    assert_eq!(*moved.segments[0].last().unwrap(), [2., LIFT_M, -1.]);
    // Asking without sharing saves nothing and leaves the caller's route alone.
    let private = home.plan("phone-a", &RouteRequest { share: Some(false), ..request(Place::Note("printer".into()), Place::Note("door".into())) })?;
    assert!(private.path.is_some());
    assert_eq!(room.saved(), vec![other.path.clone().unwrap(), moved.clone()]);
    assert!(home.clear("phone-a", &Clear { frame: "frame".into() })?);
    assert!(!home.clear("phone-a", &Clear { frame: "frame".into() })?);
    assert_eq!(room.saved(), vec![other.path.unwrap()]);
    // A route someone erased by hand does not stop the next one.
    room.paths.change("room", "frame", &room.saved()[0].id.clone(), None)?;
    assert!(home.plan("phone-b", &request(Place::Point([0., 0., 0.]), Place::Point([0.5, 0., 1.]))).is_ok());
    assert_eq!(room.saved().len(), 1);
    Ok(())
}
#[test]
fn bad_requests_are_refused() -> anyhow::Result<()> {
    let room = Room::new()?;
    let home = Navigator::new("frame", clock(), Seen(room.clone()));
    let ends = || request(Place::Note("door".into()), Place::Note("printer".into()));
    assert!(home.plan("phone", &RouteRequest { frame: "other".into(), ..ends() }).is_err(), "another frame is refused, not relabelled");
    assert!(home.plan("", &ends()).is_err());
    assert!(home.plan("phone", &request(Place::Note("gone".into()), Place::Note("printer".into()))).is_err());
    assert!(home.plan("phone", &request(Place::Point([f64::NAN, 0., 0.]), Place::Note("printer".into()))).is_err());
    assert!(home.plan("phone", &RouteRequest { agent: Some(Agent { radius_m: 0., height_m: 1.9 }), ..ends() }).is_err());
    assert!(home.clear("phone", &Clear { frame: "other".into() }).is_err());
    // Already there: nothing to draw, and nothing saved.
    let here = home.plan("phone", &request(Place::Note("door".into()), Place::Note("door".into())))?;
    assert!(here.path.is_none() && here.length_m == 0. && room.saved().is_empty());
    assert_eq!(serde_json::to_value(ends())?, serde_json::json!({"frame": "frame", "from": {"note": "door"}, "to": {"note": "printer"}}));
    assert_eq!(serde_json::from_value::<Place>(serde_json::json!({"point": [1., 2., 3.]}))?, Place::Point([1., 2., 3.]));
    assert!(serde_json::from_value::<RouteRequest>(serde_json::json!({"frame": "f", "from": {"note": "a"}, "to": {"note": "b"}, "extra": 1})).is_err());
    Ok(())
}
