use spatial_home::compact_pose::FrameRef;
use spatial_home::depth_points::*;

fn contract() -> PointsContract {
    PointsContract {
        frame: FrameRef { peer_id: "sensor-peer".into(), id: "depth-optical:boot-7".into(), hash: "f".repeat(64) },
        clock: auki_components::ClockReference { peer_id: "sensor-peer".into(), id: "capture:boot-7".into(), hash: "c".repeat(64) },
        session_id: "boot-7".into(),
    }
}

fn one_point(colors: bool) -> PackedPoints {
    PackedPoints::from_points(contract(), &Points {
        points: vec![[1., -2., 3.5]], colors: colors.then_some(vec![[10, 20, 30]]),
    }).unwrap()
}

#[test]
fn compact_binary_round_trip_keeps_declared_identity_and_measured_f32_values() {
    // BBOS's source coordinates are float32. Widening then packing them must
    // preserve every original bit, including signed zero, without float16 loss.
    let measured = [[0.12345678_f32, -0., 99.87654], [-12.125, 0.00001234567, 2.345678]];
    let source = Points { points: measured.map(|p| p.map(f64::from)).to_vec(), colors: Some(vec![[0, 127, 255], [3, 2, 1]]) };
    let packed = PackedPoints::from_points(contract(), &source).unwrap();
    assert_eq!(packed.contract(), &contract());
    assert_eq!(packed.point_count(), 2);
    assert!(packed.has_colors());
    packed.validate().unwrap();
    let wire = packed.as_bytes();
    assert_eq!(&wire[..4], b"APC2");
    assert_eq!(wire[4..8], [1, 0, 0, 0]);
    assert_eq!(u32::from_le_bytes(wire[8..12].try_into().unwrap()), 2);
    let descriptor_size = u32::from_le_bytes(wire[12..16].try_into().unwrap()) as usize;
    assert_eq!(wire.len(), PACKED_HEADER_BYTES + descriptor_size + 2 * 15);
    assert_eq!(&wire[PACKED_HEADER_BYTES + descriptor_size..PACKED_HEADER_BYTES + descriptor_size + 4], &measured[0][0].to_le_bytes());
    let decoded = PackedPoints::from_bytes(wire).unwrap().into_points();
    assert_eq!(decoded.colors, source.colors);
    for (expected, actual) in measured.iter().flatten().zip(decoded.points.iter().flatten()) {
        assert_eq!(expected.to_bits(), (*actual as f32).to_bits());
    }
}

#[test]
fn uncolored_capture_uses_only_twelve_bytes_per_point() {
    let packed = one_point(false);
    let descriptor_size = serde_json::to_vec(packed.contract()).unwrap().len();
    assert_eq!(packed.as_bytes().len(), PACKED_HEADER_BYTES + descriptor_size + 12);
    assert!(!packed.has_colors());
    let decoded = PackedPoints::from_bytes(packed.as_bytes()).unwrap().into_points();
    assert_eq!(decoded.points, vec![[1., -2., 3.5]]);
    assert_eq!(decoded.colors, None);
}

#[test]
fn packed_maximum_is_bounded_and_materially_smaller_than_json() {
    let points = Points {
        points: (0..MAX_POINTS).map(|i| [i as f64 / 123.456789, -(i as f64) / 912.345678, 0.123456789]).collect(),
        colors: Some(vec![[123, 231, 42]; MAX_POINTS]),
    };
    let packed = PackedPoints::from_points(contract(), &points).unwrap();
    assert!(packed.as_bytes().len() <= MAX_PACKED_BYTES);
    assert!(packed.as_bytes().len() * 3 < serde_json::to_vec(&points).unwrap().len());
    assert_eq!(PackedPoints::from_bytes(packed.as_bytes()).unwrap().into_points().points.len(), MAX_POINTS);
}

#[test]
fn rejects_unknown_versions_flags_and_reserved_bytes() {
    for index in [0, 1, 2, 3, 4, 5, 6, 7] {
        let mut bytes = one_point(true).as_bytes().to_vec();
        bytes[index] = 0xff;
        assert!(PackedPoints::from_bytes(&bytes).is_err(), "accepted mutation at {index}");
    }
}

#[test]
fn rejects_truncated_trailing_and_oversized_frames_before_allocating_points() {
    let bytes = one_point(true).as_bytes().to_vec();
    for length in 0..bytes.len() {
        assert!(PackedPoints::from_bytes(&bytes[..length]).is_err(), "accepted length {length}");
    }
    let mut trailing = bytes.clone(); trailing.push(0);
    assert!(PackedPoints::from_bytes(&trailing).is_err());
    assert!(PackedPoints::from_bytes(&vec![0; MAX_PACKED_BYTES + 1]).is_err());
    for (offset, value) in [(8, 0), (8, MAX_POINTS as u32 + 1), (8, u32::MAX), (12, 0), (12, MAX_PACKED_CONTRACT_BYTES as u32 + 1), (12, u32::MAX)] {
        let mut invalid = bytes.clone(); invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(PackedPoints::from_bytes(&invalid).is_err());
    }
}

#[test]
fn rejects_nonfinite_and_out_of_bounds_coordinates_on_both_sides() {
    for coordinate in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 100.001, -100.001] {
        let mut bytes = one_point(false).as_bytes().to_vec();
        let offset = PACKED_HEADER_BYTES + u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        bytes[offset..offset + 4].copy_from_slice(&coordinate.to_le_bytes());
        assert!(PackedPoints::from_bytes(&bytes).is_err());
        assert!(PackedPoints::from_points(contract(), &Points { points: vec![[coordinate as f64, 0., 1.]], colors: None }).is_err());
    }
}

#[test]
fn rejects_invalid_color_counts_empty_capture_and_unbound_contract() {
    assert!(PackedPoints::from_points(contract(), &Points { points: vec![], colors: None }).is_err());
    assert!(PackedPoints::from_points(contract(), &Points { points: vec![[0., 0., 1.]; MAX_POINTS + 1], colors: None }).is_err());
    assert!(PackedPoints::from_points(contract(), &Points { points: vec![[0., 0., 1.]], colors: Some(vec![]) }).is_err());
    let source = Points { points: vec![[0., 0., 1.]], colors: None };
    let mut c = contract(); c.session_id.clear();
    assert!(PackedPoints::from_points(c, &source).is_err());
    let mut c = contract(); c.frame.hash.clear();
    assert!(PackedPoints::from_points(c, &source).is_err());
    let mut c = contract(); c.clock.id = "x".repeat(MAX_PACKED_CONTRACT_BYTES);
    assert!(PackedPoints::from_points(c, &source).is_err());
}

#[test]
fn malformed_descriptor_is_rejected_and_v1_json_stays_unchanged() {
    let mut bytes = one_point(false).as_bytes().to_vec();
    bytes[PACKED_HEADER_BYTES] = b'!';
    assert!(PackedPoints::from_bytes(&bytes).is_err());
    let legacy = Points { points: vec![[0., 1., 2.]], colors: Some(vec![[1, 2, 3]]) };
    let json = serde_json::to_string(&legacy).unwrap();
    assert_eq!(json, r#"{"points":[[0.0,1.0,2.0]],"colors":[[1,2,3]]}"#);
    assert_eq!(serde_json::from_str::<Points>(&json).unwrap().colors, legacy.colors);
    assert_eq!(<Points as auki_components::ContractType>::DATATYPE, POINTS_SCHEMA);
    assert_ne!(PACKED_POINTS_SCHEMA, POINTS_SCHEMA);
}
