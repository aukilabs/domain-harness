import Foundation
import simd

func check(_ condition: @autoclosure () -> Bool, _ message: String) {
    if !condition() { fatalError(message) }
}

let k = simd_float3x3(SIMD3(1200, 0, 0), SIMD3(0, 1200, 0), SIMD3(960, 720, 1))
let square: [SIMD3<Float>] = [SIMD3(-0.075, 0.075, 0), SIMD3(0.075, 0.075, 0),
                            SIMD3(0.075, -0.075, 0), SIMD3(-0.075, -0.075, 0)]
let conversion = simd_float4x4(diagonal: SIMD4<Float>(1, -1, -1, 1))
for roll in [Float(0), 0.7, 1.57, 3.14] {
    let rotation = simd_quatf(angle: roll, axis: SIMD3(0, 0, 1)) *
        simd_quatf(angle: 0.3, axis: SIMD3(0, 1, 0)) * simd_quatf(angle: .pi, axis: SIMD3(1, 0, 0))
    var optical = simd_float4x4(rotation)
    optical.columns.3 = SIMD4(0.1, -0.05, 1.1, 1)
    let pixels = square.map { p -> SIMD2<Float> in
        let p4 = optical * SIMD4(p, 1)
        let pixel = k * SIMD3(p4.x, p4.y, p4.z)
        return SIMD2(pixel.x, pixel.y) / pixel.z
    }
    let solved = PortalPose.estimate(corners: pixels, side: 0.15, intrinsics: k)!
    let expected = conversion * optical
    check(simd_distance(solved.columns.3, expected.columns.3) < 0.0001, "metric translation")
    check(abs(simd_dot(simd_quatf(solved).vector, simd_quatf(expected).vector)) > 0.99999, "rotation")
    var portalInMap = simd_float4x4(simd_quatf(angle: 1.2, axis: SIMD3(0, 1, 0)))
    portalInMap.columns.3 = SIMD4(-3, 0, 2, 1)
    var cameraInAR = matrix_identity_float4x4
    cameraInAR.columns.3 = SIMD4(1, 2, 3, 1)
    let mapInAR = cameraInAR * solved * portalInMap.inverse
    check(simd_distance((mapInAR * portalInMap).columns.3, (cameraInAR * expected).columns.3) < 0.0001,
          "Home-to-AR alignment composition")
}
check(PortalPose.estimate(corners: Array(repeating: .zero, count: 4), side: 0.15, intrinsics: k) == nil,
      "degenerate corners rejected")

let frame: [String: Any] = ["id": "home", "convention": "opengl", "handedness": "right",
                          "up_axis": "Y", "meters_per_unit": 1]
let pose: [String: Any] = ["from_frame_id": "portal:ABC12345678", "to_frame_id": "home",
                         "translation": [1, 0, 2], "rotation_wxyz": [1, 0, 0, 0]]
let portal: [String: Any] = ["anchor_id": "ABC12345678", "payload": "ABC12345678",
                           "side_length_m": 0.15, "pose_in_map": pose]
func snapshot(frame: [String: Any], portal: [String: Any]) -> [String: Any] {
    ["scenegraph": ["map": ["frame": frame], "anchors": ["ABC12345678": portal]]]
}
let direct = snapshot(frame: frame, portal: portal)
for object in [direct, ["portals": direct]] {
    let map = try PortalMap.load(JSONSerialization.data(withJSONObject: object))
    check(map.scenegraph.anchors.count == 1, "direct and Home envelope import")
    check(map == (try! PortalMap.load(JSONSerialization.data(withJSONObject: direct))),
          "unchanged network Portals preserve AR alignment")
    check(map.matching("https://r8.hr/ABC12345678") != nil, "known printed payload")
    check(map.matching("https://other.example/ABC12345678") == nil, "unknown URL rejected")
    check(map.matching("https://r8.hr/ABC12345678?other=1") == nil, "ambiguous URL rejected")
}
var wrong = frame; wrong["convention"] = "ros2_body"
check((try? PortalMap.load(JSONSerialization.data(withJSONObject: snapshot(frame: wrong, portal: portal)))) == nil,
      "wrong convention rejected")
var wrongPose = pose; wrongPose["to_frame_id"] = "other"
var wrongPortal = portal; wrongPortal["pose_in_map"] = wrongPose
check((try? PortalMap.load(JSONSerialization.data(withJSONObject: snapshot(frame: frame, portal: wrongPortal)))) == nil,
      "wrong destination rejected")
wrongPortal = portal; wrongPortal["side_length_m"] = -1
check((try? PortalMap.load(JSONSerialization.data(withJSONObject: snapshot(frame: frame, portal: wrongPortal)))) == nil,
      "negative size rejected")
print("Portal viewer checks passed: pose, rotated corners, alignment, import and invalid contracts")

let pairing: [String: Any] = ["schema": "spatial-harness.home-connection/v1",
    "peer_id": "test-peer", "domain_id": "b2881524-954d-4979-b715-240ed8f02807",
    "routes": ["/ip4/192.168.9.142/tcp/4320"], "product_id": "canonical-scene",
    "frame": "domain:b2881524-954d-4979-b715-240ed8f02807:home-origin-UCOF1J40N7O"]
let loadedPairing = try HomeProfile.load(JSONSerialization.data(withJSONObject: pairing))
check(loadedPairing.frame == pairing["frame"] as! String, "current Home origin preserved")
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: direct))) == nil, "scene is not a pairing profile")
var invalidPairing = pairing; invalidPairing["frame"] = ""
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: invalidPairing))) == nil, "empty frame rejected")
print("Home pairing checks passed")
var conferencePairing = pairing
conferencePairing["frame"] = "local-scene:humanoid-hub-conference:portal-HNY3JGNLRAE"
conferencePairing["product_id"] = "local-scene:humanoid-hub-conference:mobile"
let conferenceProfile = try HomeProfile.load(JSONSerialization.data(withJSONObject: conferencePairing))
check(conferenceProfile.sceneID == "humanoid-hub-conference", "selected scene retained")
check(conferenceProfile.sceneLabel == "Humanoid Hub Conference", "selected scene visible on phone")
var mixedPairing = conferencePairing; mixedPairing["product_id"] = "local-scene:nacs:mobile"
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: mixedPairing))) == nil, "different scene Product rejected")
mixedPairing = conferencePairing; mixedPairing["product_id"] = "canonical-scene"
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: mixedPairing))) == nil, "legacy Product cannot masquerade as conference")
print("Independent scene pairing checks passed")

var cooldown = CalibrationCooldown()
check(cooldown.canScan(at: 0), "first scan is immediate")
cooldown.accept(at: 10)
check(!cooldown.canScan(at: 10.99), "no repeat calibration within one second")
check(cooldown.canScan(at: 11), "calibration resumes after one second")
cooldown.reset()
check(cooldown.canScan(at: 10.1), "explicit realign permits immediate scan")
print("Single-scan calibration cooldown checks passed")

let matchingMap = try PortalMap.load(JSONSerialization.data(withJSONObject: direct))
for prefix in ["https://r8.hr", "HTTPS://R8.HR", "Https://R8.Hr"] {
    check(matchingMap.matching(prefix + "/ABC12345678") != nil, "Galbot-compatible URL prefix casing")
}
for payload in ["HTTPS://R8.HR/abc12345678", "https://evil.example/ABC12345678",
                "https://r8.hr/ABC12345678?x=1", "https://r8.hr/ABC12345678/",
                "https://r8.hr/a/ABC12345678", "https://r8.hr/%41BC12345678"] {
    check(matchingMap.matching(payload) == nil, "unknown or malformed identity rejected")
}
print("Galbot-compatible Portal payload checks passed")

var fullScene = direct
fullScene["render"] = ["frame": "home", "voxel_size": 0.1,
    "voxels": [["center": [1,2,3], "linear_rgb": [0.2,0.3,0.4]]],
    "objects": [["id": "chair", "label": "Chair", "position": [1,0,1]]],
    "notes": [["id": "note", "text": "Hello", "position": [0,1,0]]]]
let decodedScene = try PortalMap.load(JSONSerialization.data(withJSONObject: fullScene))
check(decodedScene.render?.voxels.count == 1, "occupied geometry crosses bridge")
check(decodedScene.render?.notes.first?.text == "Hello", "scene note preserved")
check(decodedScene.scenegraph == matchingMap.scenegraph, "geometry updates preserve calibration reference")
print("Full scene decoding checks passed")

let floorHit = SceneRay.hit(origin: SIMD3(0,2,0), direction: SIMD3(0,-1,0), scene: nil)
check(floorHit == SIMD3(0,0,0), "empty scene still intersects Home floor")
check(SceneRay.hit(origin: SIMD3(0,2,0), direction: SIMD3(1,0,0), scene: nil) == nil, "parallel ray misses floor")
check(SceneRay.hit(origin: SIMD3(0,2,0), direction: SIMD3(0,1,0), scene: nil) == nil, "floor behind camera rejected")
check(SceneRay.hit(origin: SIMD3(0,2,0), direction: .zero, scene: nil) == nil, "zero direction rejected")
let voxelHit = SceneRay.hit(origin: SIMD3(1,4,3), direction: SIMD3(0,-1,0), scene: decodedScene.render)!
check(abs(voxelHit.y - 2.05) < 0.0001, "voxel in front of floor wins")
let floorFirst = SceneRay.hit(origin: SIMD3(1,-1,3), direction: SIMD3(0,1,0), scene: decodedScene.render)!
check(abs(floorFirst.y) < 0.0001, "floor in front of voxel wins")
print("Screen-ray floor and voxel checks passed")

var poseHomeInAR = simd_float4x4(simd_quatf(angle: .pi/2, axis: SIMD3(0,1,0)))
poseHomeInAR.columns.3 = SIMD4(8,0,3,1)
var expectedCameraHome = matrix_identity_float4x4
expectedCameraHome.columns.3 = SIMD4(1,1.5,2,1)
let phonePose = PhonePoseSample(session: "test", timestamp: 2, cameraInAR: poseHomeInAR * expectedCameraHome,
                               homeInAR: poseHomeInAR, homeFrame: "home")
check(simd_distance(phonePose.pose.matrix.columns.3, expectedCameraHome.columns.3) < 0.00001,
      "published phone pose converts from AR into calibrated Home coordinates")
check(phonePose.timestamp_ns == 2_000_000_000 && phonePose.pose.to_frame_id == "home", "capture clock and explicit destination survive encoding")
print("Phone pose publication checks passed")

var robotScene = fullScene
var renderWithRobot = robotScene["render"] as! [String: Any]
renderWithRobot["local_mapping"] = ["robot_pose": ["frame": "home",
    "pose": ["translation": [1,2,3], "rotation_xyzw": [0,0,0,1]]]]
robotScene["render"] = renderWithRobot
let robotDecoded = try PortalMap.load(JSONSerialization.data(withJSONObject: robotScene))
check(robotDecoded.render?.local_mapping?.robot_pose?.pose.matrix.columns.3 == SIMD4(1,2,3,1),
      "live runner quaternion contract differs from Portal quaternion contract")
print("Live runner geometry decoding checks passed")

let depthIntrinsics = simd_float3x3(columns: (SIMD3(1000,0,0), SIMD3(0,1000,0), SIMD3(960,720,1)))
let centerDepth = DepthProjection.point(x: 128, y: 96, depth: 2, width: 256, height: 192,
                                      imageWidth: 1920, imageHeight: 1440, intrinsics: depthIntrinsics)!
check(simd_distance(SIMD3(centerDepth[0],centerDepth[1],centerDepth[2]), SIMD3(0,0,-2)) < 0.00001,
      "depth uses scaled intrinsics and looks along camera negative Z")
let cornerDepth = DepthProjection.point(x: 160, y: 64, depth: 2, width: 256, height: 192,
                                      imageWidth: 1920, imageHeight: 1440, intrinsics: depthIntrinsics)!
check(abs(cornerDepth[0]-0.48)<0.00001 && abs(cornerDepth[1]-0.48)<0.00001,
      "image right/up becomes camera right/up without display rotation")
for invalid: Float in [0, -1, 6, .nan, .infinity] {
    check(DepthProjection.point(x:128,y:96,depth:invalid,width:256,height:192,imageWidth:1920,imageHeight:1440,intrinsics:depthIntrinsics)==nil,
          "invalid and out-of-range depth rejected")
}
print("Phone depth projection checks passed")

var ownMap = PhoneLocalVoxels()
ownMap.add(SIMD3(1, -2, 3)); ownMap.add(SIMD3(1, -2, 3)); ownMap.add(SIMD3(0, 0, 10_001))
check(ownMap.cells == [[1, -2, 3]], "a cell is kept once, and cells Home would refuse are left out")
for x in 0..<Int32(PhoneLocalVoxels.limit) { ownMap.add(SIMD3(x, 5, 0)) }
check(ownMap.cells.count == PhoneLocalVoxels.limit && ownMap.cells[1] == [0, 5, 0], "a full map takes no new cells")
let ownJSON = try JSONSerialization.jsonObject(with: JSONEncoder().encode(
    PhoneLocalMap(session: "s", timestamp_ns: 7, cells: [[1, -2, 3]]))) as! [String: Any]
check(Set(ownJSON.keys) == ["session", "timestamp_ns", "cells"], "Home refuses a local map with any other field")
print("Phone own map checks passed")

func deviceNote(_ x: Float, frame: String = "home", phone: String? = nil, state: String? = nil, peer: String = "p1") -> RenderScene.Note {
    let key = phone == nil ? "robot_pose" : "phone_pose"
    let from = phone.map { "phone-camera:" + $0 } ?? "robot.base"
    let extra = state.map { ",\"state\":\"\($0)\"" } ?? ""
    let json = "{\"id\":\"n\",\"text\":\"Bracketbot 182\",\"position\":[\(x),0,0],\"\(key)\":{\"peer_id\":\"\(peer)\",\"name\":\"Bracketbot 182\"\(extra),\"pose\":{\"from_frame_id\":\"\(from)\",\"to_frame_id\":\"\(frame)\",\"translation\":[\(x),0,0],\"rotation_wxyz\":[1,0,0,0]}}}"
    return try! JSONDecoder().decode(RenderScene.Note.self, from: Data(json.utf8))
}
var trails = DeviceTrails()
let first = trails.record([deviceNote(0)], frame: "home")
check(first.first?.trail.count == 1 && first.first?.id == "robot:p1" && first.first?.kind == .robot, "a robot's first position starts its trail")
check(trails.record([deviceNote(0.02)], frame: "home").first?.trail.count == 1, "a parked robot adds nothing to its trail")
let moved = trails.record([deviceNote(0.5)], frame: "home")
check(moved.first?.trail.count == 2 && moved.first?.name == "Bracketbot 182" && moved.first?.position.x == 0.5
      && abs((moved.first?.travelled ?? 0) - 0.5) < 1e-5, "a robot that moved extends its trail and keeps Home's name")
let seenAt = Date(timeIntervalSince1970: 100)
_ = trails.record([deviceNote(0.5)], frame: "home", now: seenAt)
check(trails.record([], frame: "home", now: Date(timeIntervalSince1970: 103)).first?.state == .live,
      "one skipped report does not make a robot lost")
let gone = trails.record([], frame: "home", now: Date(timeIntervalSince1970: 160))
check(gone.count == 1 && gone[0].state == .lost && gone[0].position.x == 0.5 && gone[0].trail.count == 2 && gone[0].lastSeen == seenAt,
      "a robot Home stops reporting stays at its last position, lost, with the time it was last seen")
check(trails.record([deviceNote(0.5)], frame: "home").first?.state == .live, "a robot that comes back is live again")
check(trails.record([deviceNote(1, frame: "other")], frame: "home", now: Date(timeIntervalSinceNow: 60)).allSatisfy { $0.state == .lost },
      "a pose in another frame is not placed")
check(trails.record([deviceNote(1, frame: "other")], frame: "other").first?.trail.count == 1, "a new frame starts a new trail")
var phones = DeviceTrails()
let phone = phones.record([deviceNote(1, phone: "s1"), deviceNote(2, phone: "mine", peer: "me")], frame: "home", ownFrame: "phone-camera:mine")
check(phone.count == 1 && phone[0].id == "phone:p1" && phone[0].kind == .phone && phone[0].state == .live,
      "other phones are tracked; this phone is not")
check(phones.record([deviceNote(1, phone: "s1", state: "asleep")], frame: "home").first?.state == .asleep, "a phone that says it is asleep is asleep")
var relaunched = DeviceTrails()
check(relaunched.record([deviceNote(1, phone: "old")], frame: "home", ownFrame: "phone-camera:new").count == 1,
      "before Home lists the new session, the old one looks like another phone")
check(relaunched.record([deviceNote(1, phone: "new")], frame: "home", ownFrame: "phone-camera:new").isEmpty
      && relaunched.record([deviceNote(1, phone: "old")], frame: "home", ownFrame: "phone-camera:new").isEmpty,
      "once Home lists this phone, its earlier session is not shown as a lost phone")
check(DeviceTrails.color(age: 0).z > DeviceTrails.color(age: 0).x && DeviceTrails.color(age: 1).x > DeviceTrails.color(age: 1).z,
      "old trail is blue, new trail is yellow")
let oldNote = try JSONDecoder().decode(RenderScene.Note.self, from: Data("{\"id\":\"n\",\"text\":\"t\",\"position\":[0,0,0]}".utf8))
check(oldNote.robot_pose == nil && oldNote.phone_pose == nil, "notes without a pose still decode")
print("Device trail checks passed")

let fix = GeoFix(latitude: 37, longitude: -122, altitude: 10, accuracy: 5)
let heading = GeoReference(origin: fix, north: SIMD2(0, -1))
let northward = heading.coordinate(of: SIMD3(0, 2, -111.3195))
check(abs(northward.latitude - 37.001) < 1e-7 && abs(northward.longitude + 122) < 1e-9 && northward.altitude == 12,
      "a heading-aligned frame has north along -Z and height along Y")
let eastward = heading.coordinate(of: SIMD3(100, 0, 0))
check(abs(eastward.longitude - (-122 + 100 / (111_319.49 * cos(37 * Double.pi / 180)))) < 1e-7 && eastward.latitude == 37,
      "east is +X, and a degree of longitude shrinks with latitude")
check(heading.bearing(of: SIMD2(0, -1)) == 0 && abs(heading.bearing(of: SIMD2(1, 0)) - 90) < 1e-4
      && abs(heading.bearing(of: SIMD2(-1, 0)) - 270) < 1e-4, "bearings run clockwise from north")
let held = GeoReference(fix: fix, at: SIMD3(3, 1, -4), north: SIMD2(0, -1)).coordinate(of: SIMD3(3, 1, -4))
check(abs(held.latitude - 37) < 1e-9 && abs(held.longitude + 122) < 1e-9 && abs(held.altitude - 10) < 1e-5,
      "a fix taken away from the origin is still the coordinate of where it was taken")
var room = simd_float4x4(simd_quatf(angle: 1.1, axis: SIMD3(0, 1, 0)))
room.columns.3 = SIMD4(7, 0.5, -3, 1)
let inRoom = heading.expressed(in: room)
let spot = SIMD4<Float>(2, 1, 5, 1), seen = room * spot
let geoDirect = heading.coordinate(of: SIMD3(seen.x, seen.y, seen.z)), geoThrough = inRoom.coordinate(of: SIMD3(spot.x, spot.y, spot.z))
check(abs(geoDirect.latitude - geoThrough.latitude) < 1e-9 && abs(geoDirect.longitude - geoThrough.longitude) < 1e-9
      && abs(geoDirect.altitude - geoThrough.altitude) < 1e-4, "a point has one coordinate, whichever frame it is measured in")
let back = inRoom.expressed(in: room.inverse)
check(simd_distance(back.north, heading.north) < 1e-5 && abs(back.origin.latitude - 37) < 1e-9 && abs(back.origin.longitude + 122) < 1e-9,
      "a reference carried into a frame and back is unchanged")
var gate = HeadingGate()
check(gate.step(at: 0, tracking: true, headingError: 10, hasFix: true) == .waiting
      && gate.step(at: 1.9, tracking: true, headingError: 10, hasFix: true) == .waiting
      && gate.step(at: 2, tracking: true, headingError: 10, hasFix: true) == .established, "north is taken once the compass has held steady")
gate = HeadingGate()
_ = gate.step(at: 0, tracking: true, headingError: 10, hasFix: true)
_ = gate.step(at: 1.5, tracking: true, headingError: 45, hasFix: true)
check(gate.step(at: 3, tracking: true, headingError: 10, hasFix: true) == .waiting
      && gate.step(at: 5, tracking: true, headingError: 10, hasFix: true) == .established, "a compass that wavers starts the wait again")
gate = HeadingGate()
check(gate.step(at: 0, tracking: true, headingError: nil, hasFix: true) == .waiting
      && gate.step(at: 14, tracking: false, headingError: 5, hasFix: true) == .waiting
      && gate.step(at: 14.5, tracking: true, headingError: 5, hasFix: false) == .waiting
      && gate.step(at: 15, tracking: true, headingError: 90, hasFix: true) == .gaveUp, "without a compass, tracking and a fix, the search gives up")
print("Geo reference checks passed")
