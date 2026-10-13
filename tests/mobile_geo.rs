use spatial_home::mobile_geo::{Reference, load, merge, save};

fn reference(accuracy_m: f64, heading_error_deg: f64) -> Reference {
    Reference {
        frame: "home".into(),
        latitude: 37.,
        longitude: -122.,
        altitude_m: 10.,
        accuracy_m,
        north: [0., -1.],
        heading_error_deg,
    }
}

#[test]
fn validates_scene_coordinate_and_north() {
    let good = reference(5., 10.);
    assert!(good.validate("home").is_ok());
    assert!(good.validate("other").is_err(), "another scene's frame");
    for bad in [
        Reference { latitude: 91., ..good.clone() },
        Reference { longitude: f64::NAN, ..good.clone() },
        Reference { accuracy_m: -1., ..good.clone() },
        Reference { heading_error_deg: 200., ..good.clone() },
        Reference { north: [0.5, 0.5], ..good.clone() },
        Reference { north: [f64::NAN, 1.], ..good.clone() },
    ] {
        assert!(bad.validate("home").is_err());
    }
}

#[test]
fn keeps_the_best_position_and_the_best_north_apart() {
    let first = reference(20., 15.);
    assert_eq!(merge(None, &first), Some(first.clone()), "the first report is taken whole");
    assert_eq!(merge(Some(&first), &first), None, "the same report changes nothing");
    let better_fix = Reference { latitude: 37.001, north: [1., 0.], ..reference(5., 30.) };
    let merged = merge(Some(&first), &better_fix).unwrap();
    assert_eq!((merged.latitude, merged.accuracy_m), (37.001, 5.));
    assert_eq!((merged.north, merged.heading_error_deg), ([0., -1.], 15.), "a worse compass does not move north");
    let better_north = Reference { latitude: 38., north: [-1., 0.], ..reference(50., 3.) };
    let merged = merge(Some(&merged), &better_north).unwrap();
    assert_eq!((merged.latitude, merged.accuracy_m), (37.001, 5.), "a worse fix does not move the scene");
    assert_eq!((merged.north, merged.heading_error_deg), ([-1., 0.], 3.));
    assert_eq!(merge(Some(&merged), &first), None, "a worse report on both counts is ignored");
    let elsewhere = Reference { frame: "other".into(), ..first.clone() };
    assert_eq!(merge(Some(&merged), &elsewhere), Some(elsewhere.clone()), "a new frame starts again");
}

#[test]
fn survives_a_restart_only_for_its_own_scene() {
    let directory = std::env::temp_dir().join(format!("mobile-geo-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mobile-geo").join("scene.json");
    assert_eq!(load(&path, "home"), None);
    save(&path, &reference(5., 10.)).unwrap();
    assert_eq!(load(&path, "home"), Some(reference(5., 10.)));
    assert_eq!(load(&path, "other"), None);
    std::fs::remove_dir_all(directory).unwrap();
}
