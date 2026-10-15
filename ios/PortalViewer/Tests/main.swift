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
// Scenes whose origin is the first observed Portal name it `:origin`.
var originPairing = pairing
originPairing["frame"] = "local-scene:bb-lab:origin"
originPairing["product_id"] = "local-scene:bb-lab:mobile"
let originProfile = try HomeProfile.load(JSONSerialization.data(withJSONObject: originPairing))
check(originProfile.sceneID == "bb-lab", "origin-named scene retained")
check(originProfile.sceneLabel == "Bb Lab", "origin-named scene visible on phone")
var epochPairing = originPairing
epochPairing["frame"] = "local-scene:bb-lab:epoch-6f1c2a4e-0b7d-4c55-9a51-2f3e8d7b9c10"
let epochProfile = try HomeProfile.load(JSONSerialization.data(withJSONObject: epochPairing))
check(epochProfile.sceneID == "bb-lab", "epoch-named scene retained")
epochPairing["frame"] = "local-scene:bb-lab:epoch-not-a-uuid"
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: epochPairing))) == nil, "malformed epoch rejected")
var badOrigin = originPairing; badOrigin["frame"] = "local-scene:BB_Lab:origin"
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: badOrigin))) == nil, "invalid scene ID rejected")
badOrigin = originPairing; badOrigin["product_id"] = "local-scene:nacs:mobile"
check((try? HomeProfile.load(JSONSerialization.data(withJSONObject: badOrigin))) == nil, "origin scene Product must match")
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

func note(_ id: String, _ text: String = "Hi") -> RenderScene.Note {
    RenderScene.Note(id: id, text: text, position: [1, 0, -2], phone_pose: nil)
}
var inbox = MessageInbox()
check(inbox.record([note("a"), note("b")], frame: "home").isEmpty && inbox.unread == 0 && inbox.messages.map(\.id) == ["a", "b"],
      "what a scene already holds is listed in order and is not news")
check(inbox.record([note("a"), note("b"), note("c"), note("d")], frame: "home", mine: ["d"]).map(\.id) == ["c"] && inbox.unread == 1,
      "a note from someone else is news; this phone's own is not")
check(inbox.record([note("a"), note("b"), note("c", "Edited"), note("d")], frame: "home").isEmpty && inbox.unread == 1
      && inbox.messages[2].text == "Edited", "a message stays unread, and follows Home's edits, until it is opened")
inbox.open("c")
check(inbox.unread == 0 && inbox.record([note("b"), note("c")], frame: "home").isEmpty && inbox.messages.map(\.id) == ["b", "c"],
      "an opened message is read and a deleted one leaves the list")
check(inbox.record([note("z")], frame: "other").isEmpty && inbox.unread == 0, "another scene starts again with nothing new")
let phoneLabel = RenderScene.Note(id: "p", text: "Phone", position: [0, 0, 0],
    phone_pose: RenderScene.PhonePose(peer_id: "p", name: nil, state: nil, pose: decodedScene.scenegraph.anchors.values.first!.pose_in_map))
check(inbox.record([note("z"), phoneLabel], frame: "other").isEmpty && inbox.messages.count == 1, "Home's device labels are not messages")
// A 90 degree lens on a square screen.
let lens = simd_float4x4(SIMD4(1, 0, 0, 0), SIMD4(0, 1, 0, 0), SIMD4(0, 0, -1, -1), SIMD4(0, 0, -0.02, 0))
let ahead = MessageAim(spot: SIMD3(0.5, 0.5, -2), lens: lens)
check(ahead.screen == SIMD2(0.625, 0.375) && ahead.hint == "Here", "a spot in view is marked on the screen, up being towards the top")
check(MessageAim(spot: SIMD3(3, 0, -1), lens: lens).hint == "Turn right" && MessageAim(spot: SIMD3(-3, 0, -1), lens: lens).hint == "Turn left"
      && MessageAim(spot: SIMD3(0, 3, -1), lens: lens).hint == "Tilt up" && MessageAim(spot: SIMD3(0, -3, -1), lens: lens).hint == "Tilt down",
      "a spot out of view says which way to turn")
let behind = MessageAim(spot: SIMD3(-0.2, 0.1, 2), lens: lens)
check(behind.screen == nil && behind.hint == "Turn around" && behind.turn < 0, "a spot behind is never on screen, and turns the nearer way")
check(abs(MessageAim(spot: SIMD3(3, 0, -1), lens: lens).turn - .pi / 2) < 0.001 && abs(MessageAim(spot: SIMD3(0, 0, 4), lens: lens).distance - 4) < 0.001,
      "the arrow points across the screen and the distance is in metres")
print("Message checks passed")

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
let phonePose = PhonePoseSample(session: "test", timestamp: 2, cameraInAR: poseHomeInAR * expectedCameraHome)
check(simd_distance(phonePose.pose.matrix.columns.3, (poseHomeInAR * expectedCameraHome).columns.3) < 0.00001,
      "published phone pose stays in native AR coordinates")
check(phonePose.timestamp_ns == 2_000_000_000 && phonePose.pose.to_frame_id == "phone-ar:test", "capture clock and native destination survive encoding")
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

var crayon = CrayonDraft()
check(crayon.add(SIMD3(0, 0, 0)) == nil && crayon.add(SIMD3(0.01, 0, 0)) == nil, "points closer than 2 cm add no line")
check(crayon.add(SIMD3(0.05, 0, 0)) != nil && crayon.stroke(id: "a")?.segments == [[[0, 0, 0], [0.05, 0, 0]]], "a stroke is its spaced points")
_ = crayon.add(nil); _ = crayon.add(SIMD3(1, 0, 0))
check(crayon.segments.count == 2 && crayon.stroke(id: "a")?.segments.count == 1, "a miss starts a new segment, and a lone point is not saved")
check(CrayonDraft().stroke(id: "a") == nil, "nothing drawn, nothing saved")
let bend = [SIMD3<Float>(0, 0, 0), SIMD3(0.1, 0, 0), SIMD3(0.1, 0, 0.1)]
let curve = CrayonTube.smooth(bend)
check(curve.count == 9 && curve[0] == bend[0] && simd_distance(curve[4], bend[1]) < 0.0001 && curve[8] == bend[2],
      "the smooth line still passes through every drawn point")
let tube = CrayonTube.mesh(bend, radius: 0.02)!
check(tube.vertices.count == 9 * 8 + 2 && tube.indices.count == 8 * 8 * 6 + 2 * 8 * 3 && tube.indices.allSatisfy { $0 < UInt32(tube.vertices.count) },
      "a tube is one closed skin of rings with two ends")
check(tube.vertices.prefix(72).enumerated().allSatisfy { abs(simd_distance($1, curve[$0 / 8]) - 0.02) < 0.0001 },
      "every ring sits one radius from the line")
check(tube.along.count == tube.vertices.count && tube.along.first?.x == 0
      && zip(tube.along.prefix(64), tube.along.dropFirst(8).prefix(64)).allSatisfy { $0.x < $1.x },
      "the texture runs along the line, from where it was started")
check(CrayonTube.mesh([bend[0]], radius: 0.02) == nil && CrayonTube.mesh([bend[0], bend[0]], radius: 0.02) == nil, "a point is not a line")
print("Crayon stroke checks passed")

// A wall across the way at body height, with a gap at its far end, on floor the map has seen.
var walled = [SIMD3<Float>]()
for x in stride(from: Float(-2), to: 2, by: 0.1) { for z in stride(from: Float(-1), to: 3, by: 0.1) { walled.append(SIMD3(x + 0.05, 0.05, z + 0.05)) } }
for x in stride(from: Float(-2), to: 1, by: 0.1) { for y in stride(from: Float(0.35), to: 1.5, by: 0.1) { walled.append(SIMD3(x + 0.05, y, 1.05)) } }
let walkway = MessageRoute.plan(from: SIMD3(-1, 1.4, 0), to: SIMD3(-1, 1.2, 2), voxels: walled, size: 0.1)
check(walkway.count > 2 && walkway.first == SIMD3(-1, MessageRoute.lift, 0) && walkway.last == SIMD3(-1, MessageRoute.lift, 2), "the way starts under the phone and ends under the message")
check(walkway.contains { $0.x > 1.2 } && zip(walkway, walkway.dropFirst()).allSatisfy { a, b in
    (0...20).allSatisfy { step in
        let p = a + (b - a) * (Float(step) / 20)
        return !(p.x < 1.0 && abs(p.z - 1.05) < 0.05)
    } }, "the way goes round the wall through the gap, never through it")
check(MessageRoute.plan(from: SIMD3(0, 1, 0), to: SIMD3(1, 1, 1), voxels: [], size: 0.1).count == 2, "with nothing known in the way it is the straight line")
check(MessageRoute.plan(from: SIMD3(0, 1, 0), to: SIMD3(900, 1, 0), voxels: walled, size: 0.1).count == 2, "too far to plan across is the straight line")
let crumbTrail = MessageRoute.crumbs([SIMD3(0, 0, 0), SIMD3(1, 0, 0), SIMD3(1, 0, 1)], every: 0.4)
check(crumbTrail.count == 5 && abs(crumbTrail[0].x - 0.4) < 0.0001 && abs(crumbTrail[2].z - 0.2) < 0.0001 && crumbTrail.last == SIMD3(1, 0, 1), "crumbs are evenly spaced round a corner and end on the spot")
check(MessageRoute.ahead([SIMD3(0, 0, 0), SIMD3(1, 0, 0), SIMD3(1, 0, 1)], by: 1.5) == SIMD3(1, 0, 0.5) && MessageRoute.ahead([SIMD3(0, 0, 0), SIMD3(1, 0, 0)], by: 9) == SIMD3(1, 0, 0), "the next spot to head for is along the way, not past its end")
let corner = [SIMD3<Float>(0, 0, 0), SIMD3(2, 0, 0), SIMD3(2, 0, 2)]
check(abs(MessageRoute.stray(SIMD3(1, 1.4, 0.5), from: corner) - 0.5) < 0.0001 && abs(MessageRoute.stray(SIMD3(3, 0, 3), from: corner) - 1.4142) < 0.001,
      "how far off the way a spot is, across the floor, ignoring its height")
let shared = MessageRoute.path(corner, id: "a", to: " the printer ")!
check(shared.title == "Way to the printer" && shared.segments.count == 1 && shared.segments[0].first == [0, 0, 0] && shared.segments[0].last == [2, 0, 2]
      && CrayonStroke.widths.contains(shared.width_m), "a way is a path like any drawn one, from its start to its end")
check(MessageRoute.path([SIMD3(0, 0, 0), SIMD3(0.01, 0, 0)], id: "a", to: "x") == nil, "already there: nothing to draw")
let between = MessageRoute.path(corner, id: "b", to: "the printer", from: " the door ")!
check(between.title == "Way from the door to the printer" && between.segments == shared.segments, "a way between two messages says where it starts")
check(abs(MessageRoute.length(corner) - 4) < 0.0001 && MessageRoute.length([SIMD3(1, 0, 1)]) == 0, "a way is as long as its stretches together")
check(MessageRoute.apart(corner, corner) < 0.0001 && abs(MessageRoute.apart(corner, [SIMD3(0, 0, 0), SIMD3(2, 0, 2)]) - 1.4142) < 0.05
      && MessageRoute.apart([SIMD3(0, 0, 0), SIMD3(2, 0, 2)], corner) == MessageRoute.apart(corner, [SIMD3(0, 0, 0), SIMD3(2, 0, 2)]),
      "two ways are as far apart as the farthest either gets from the other")
var pairBox = MessageInbox()
func pairNote(_ id: String, _ text: String, _ x: Float) -> RenderScene.Note {
    try! JSONDecoder().decode(RenderScene.Note.self, from: JSONSerialization.data(withJSONObject: ["id": id, "text": text, "position": [x, 1, 0]]))
}
_ = pairBox.record([pairNote("d", "door", 0), pairNote("p", "printer", 3)], frame: "f")
let picked = MessageInbox.Pair(from: pairBox.messages[0], to: pairBox.messages[1])
_ = pairBox.record([pairNote("d", "door", 0), pairNote("p", "printer", 5)], frame: "f")
check(pairBox.current(picked)?.to.position.x == 5 && pairBox.current(picked)?.from == picked.from, "the two being joined follow Home's edits")
_ = pairBox.record([pairNote("d", "door", 0)], frame: "f")
check(pairBox.current(picked) == nil, "with either gone there is no way between them")
struct RouteAsk: Encodable { let from: RoutePlace; let to: RoutePlace }
let askJSON = try! JSONSerialization.jsonObject(with: JSONEncoder().encode(RouteAsk(from: .point(SIMD3(1, 2, 3)), to: .note("n")))) as! [String: [String: Any]]
check(askJSON["from"]?["point"] as? [Double] == [1, 2, 3] && askJSON["to"]?["note"] as? String == "n" && askJSON["to"]?.count == 1,
      "an end of a route is a point or a note, as Home reads it")
let homeRoute = try! JSONDecoder().decode(HomeRoute.self, from: Data("""
{"frame":"f","status":"found","length_m":3.5,"map_timestamp_ns":7,"clock":{"peer_id":"p","id":"c","hash":"h"},
 "path":{"id":"a","title":"Way to x","color":"#ff5fa8","width_m":0.05,"segments":[[[0,0.06,0],[1,0.06,0],[1,0.06,2.5]]]}}
""".utf8))
check(homeRoute.points == [SIMD3(0, 0.06, 0), SIMD3(1, 0.06, 0), SIMD3(1, 0.06, 2.5)] && homeRoute.path?.id == "a" && homeRoute.length_m == 3.5,
      "Home's route is read as the points of its path")
let noRoute = try! JSONDecoder().decode(HomeRoute.self, from: Data(#"{"frame":"f","status":"unreachable","length_m":0,"map_timestamp_ns":7,"clock":{}}"#.utf8))
check(noRoute.points.isEmpty && noRoute.status == "unreachable", "no way through is a route without a path")
print("Message route checks passed")

let firstPortal = try JSONSerialization.jsonObject(with: JSONEncoder().encode(PortalReport(
    session: "s", timestamp_ns: 7, code: "NV16FWJ9G2X", side_length_m: PortalReport.assumedSide,
    portal_to_session: .transform(matrix_identity_float4x4, from: "portal:NV16FWJ9G2X:local", to: "phone-ar:s"),
    alignment: nil))) as! [String: Any]
check(firstPortal["alignment"] == nil && firstPortal["replace"] == nil && firstPortal["code"] as? String == "NV16FWJ9G2X",
      "a scene's first Portal is reported with no alignment at all")
print("First Portal report checks passed")

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

// A robot label as Home relays it, with the runner's joint angles and motor health.
let robotLabel = """
{"id":"robot-name:r1","text":"Bracketbot 184","position":[1,0,2],"height_m":0,"robot_pose":{"peer_id":"r1","name":"Bracketbot 184",
 "pose":{"from_frame_id":"robot.base","to_frame_id":"home","translation":[1,0,2],"rotation_wxyz":[1,0,0,0]},
 "joints":{"timestamp_ns":5,"positions":{"rj1":0.4}},
 "health":{"timestamp_ns":6,"joints":{"rj1":{"temperature_c":36.5,"current_a":0.8},"rj5":{"temperature_c":47,"current_a":1.2}},
           "battery":{"voltage":24.1,"percent":18}},
 "native_pose":{},"received_age_ms":120}}
"""
let robotNote = try JSONDecoder().decode(RenderScene.Note.self, from: Data(robotLabel.utf8))
check(robotNote.robot_pose?.joints?.positions["rj1"] == 0.4, "robot joint angles decode from Home's label")
check(robotNote.robot_pose?.health?.battery?.percent == 18 && robotNote.robot_pose?.health?.joints?.count == 2,
      "robot health decodes from Home's label")
check(deviceNote(0).robot_pose?.health == nil, "a robot without health still decodes")
let hottest = robotNote.robot_pose?.health?.warning
check(hottest?.joint == "rj5" && hottest?.level == .warm, "the hottest joint above 42 C is called out")
check(RobotHealth(timestamp_ns: 0, joints: ["rj1": .init(temperature_c: 40, current_a: 0)]).warning == nil,
      "joints at or below 42 C raise no warning")
check(RobotHealth.level(celsius: 50) == .hot && RobotHealth.level(celsius: 42) == .ok && RobotHealth.level(celsius: 42.5) == .warm,
      "temperature levels match the Home viewer")
check(RobotHealth.level(batteryPercent: 50) == .ok && RobotHealth.level(batteryPercent: 19.9) == .hot
      && RobotHealth.level(batteryPercent: 20) == .warm, "battery levels match the Home viewer")
check(RobotHealth.color(celsius: 20) == RobotHealth.color(celsius: 35) && RobotHealth.color(celsius: 60) == RobotHealth.color(celsius: 50),
      "temperature colour is green when cool and red at the limit")
print("Robot health checks passed")

// The phone places joints exactly as the Home viewer's URDF model does
// (Tests/robot-joints.json comes from web/robot-model.js; see robot-joints-fixture.mjs).
let kinematics = try RobotKinematics.load(Data(contentsOf: URL(fileURLWithPath: "../../web/robot/robot.urdf")))
struct JointFixture: Decodable { let angles: [String: Double]; let positions: [String: [Double]] }
let fixtures = try JSONDecoder().decode([String: JointFixture].self,
                                        from: Data(contentsOf: URL(fileURLWithPath: "Tests/robot-joints.json")))
for (label, fixture) in fixtures {
    let frames = kinematics.frames(fixture.angles)
    check(Set(frames.keys) == Set(fixture.positions.keys), "\(label): same movable joints as the Home viewer")
    for (name, expected) in fixture.positions {
        let p = frames[name]!.columns.3
        check(simd_distance(SIMD3(p.x, p.y, p.z), SIMD3(expected[0], expected[1], expected[2])) < 1e-4,
              "\(label): \(name) is where the Home viewer puts it")
    }
}
let atRest = kinematics.frames([:]), clamped = kinematics.frames(["rj5": 100])
check(clamped["rj6"] == kinematics.frames(["rj5": 2.094395])["rj6"], "joint angles are clamped to the URDF limits")
check(kinematics.frames(["left_left_gripper": 0.01])["left_right_gripper"] != atRest["left_right_gripper"],
      "a mimic joint follows its leader")
print("Robot kinematics checks passed")
