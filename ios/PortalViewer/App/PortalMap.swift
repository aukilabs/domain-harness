import Foundation
import simd

struct ViewerError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

struct Portal: Decodable, Equatable {
    let anchor_id: String
    let payload: String
    let side_length_m: Float
    let pose_in_map: Pose

    struct Pose: Codable, Equatable {
        let from_frame_id: String
        let to_frame_id: String
        let translation: [Float]
        let rotation_wxyz: [Float]
        var matrix: simd_float4x4 {
            let q = rotation_wxyz
            var result = simd_float4x4(simd_quatf(ix: q[1], iy: q[2], iz: q[3], r: q[0]))
            result.columns.3 = SIMD4(translation[0], translation[1], translation[2], 1)
            return result
        }
    }
}

struct PortalMap: Decodable, Equatable {
    let scenegraph: Scene
    let render: RenderScene?
    struct Scene: Decodable, Equatable {
        let map: Definition
        let anchors: [String: Portal]
    }
    struct Definition: Decodable, Equatable {
        let frame: Frame
    }
    struct Frame: Decodable, Equatable {
        let id: String
        let convention: String
        let handedness: String
        let up_axis: String
        let meters_per_unit: Float
    }
    private struct Envelope: Decodable { let portals: PortalMap }

    static func load(_ data: Data) throws -> PortalMap {
        guard data.count <= 32 * 1024 * 1024 else { throw ViewerError("Snapshot is too large") }
        let decoder = JSONDecoder()
        let map: PortalMap
        if let envelope = try? decoder.decode(Envelope.self, from: data) {
            map = envelope.portals
        } else {
            map = try decoder.decode(PortalMap.self, from: data)
        }
        let frame = map.scenegraph.map.frame
        guard !frame.id.isEmpty, frame.convention == "opengl", frame.handedness == "right",
              frame.up_axis == "Y", frame.meters_per_unit == 1 else {
            throw ViewerError("Expected a declared OpenGL, Y-up map in metres")
        }
        guard (1...1024).contains(map.scenegraph.anchors.count) else {
            throw ViewerError("Snapshot must contain 1–1024 Portals")
        }
        var payloads = Set<String>()
        for (id, portal) in map.scenegraph.anchors {
            let pose = portal.pose_in_map
            guard id == portal.anchor_id, !id.isEmpty, !pose.from_frame_id.isEmpty,
                  !portal.payload.isEmpty, payloads.insert(portal.payload).inserted,
                  pose.to_frame_id == frame.id, pose.translation.count == 3,
                  pose.rotation_wxyz.count == 4,
                  pose.translation.allSatisfy(\.isFinite), pose.rotation_wxyz.allSatisfy(\.isFinite),
                  portal.side_length_m.isFinite, portal.side_length_m > 0,
                  abs(pose.rotation_wxyz.reduce(0) { $0 + $1 * $1 } - 1) < 0.001 else {
                throw ViewerError("Invalid size, pose or frame for Portal \(id)")
            }
        }
        return map
    }

    func matching(_ payload: String) -> Portal? {
        // Only the explicitly supported printed Portal URL is mapped to a short code.
        // No backend lookup or guessed UUID mapping.
        if let exact = scenegraph.anchors.values.first(where: { $0.payload == payload }) { return exact }
        guard let code = Self.portalCode(payload) else { return nil }
        return scenegraph.anchors[code]
    }

    static func portalCode(_ payload: String) -> String? {
        // Match the Galbot's local_mapper::portal_code contract exactly:
        // URL prefix is case-insensitive; the eleven-character identity is not.
        guard let slash = payload.lastIndex(of: "/"),
              payload[..<slash].lowercased() == "https://r8.hr" else { return nil }
        let code = String(payload[payload.index(after: slash)...])
        guard code.utf8.count == 11,
              code.utf8.allSatisfy({ (65...90).contains($0) || (48...57).contains($0) }) else { return nil }
        return code
    }
}


struct HomeProfile: Decodable {
    let schema: String
    let peer_id: String
    let domain_id: String
    let routes: [String]
    let frame: String
    let product_id: String

    static func load(_ data: Data) throws -> HomeProfile {
        guard data.count <= 32 * 1024 else { throw ViewerError("Home profile is too large") }
        let value: HomeProfile
        do { value = try JSONDecoder().decode(HomeProfile.self, from: data) }
        catch { throw ViewerError("Select Home's runner connection JSON, not a scene snapshot") }
        guard value.schema == "spatial-harness.home-connection/v1",
              value.validSceneContract else {
            throw ViewerError("Unsupported Home connection profile version or Product")
        }
        guard UUID(uuidString: value.domain_id) != nil, !value.peer_id.isEmpty,
              (1...16).contains(value.routes.count), value.routes.allSatisfy({ !$0.isEmpty }),
              !value.frame.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ViewerError("Home profile is missing a valid Domain, peer, route or frame")
        }
        // Frame IDs identify physical origins; they are not fixed UI constants.
        // Rust validates supported frames before login and exact scene/profile
        // agreement on receipt. PortalMap separately checks OpenGL metre units.
        return value
    }

    var sceneID: String? {
        guard frame.hasPrefix("local-scene:") else { return nil }
        let parts = String(frame.dropFirst("local-scene:".count)).components(separatedBy: ":portal-")
        guard parts.count == 2, (1...64).contains(parts[0].utf8.count),
              parts[0].utf8.allSatisfy({ (97...122).contains($0) || (48...57).contains($0) || $0 == 45 }),
              parts[1].utf8.count == 11,
              parts[1].utf8.allSatisfy({ (65...90).contains($0) || (48...57).contains($0) }) else { return nil }
        return parts[0]
    }
    var validSceneContract: Bool {
        if let id = sceneID { return product_id == "local-scene:\(id):mobile" }
        return !frame.hasPrefix("local-scene:") && product_id == "canonical-scene"
    }
    var sceneLabel: String { sceneID?.replacingOccurrences(of: "-", with: " ").capitalized ?? "Xmart" }
}

struct RenderScene: Decodable, Equatable {
    let frame: String
    let voxel_size: Float
    let voxels: [Voxel]
    let objects: [ObjectLabel]
    let notes: [Note]
    var volumes: [SemanticVolume]? = nil
    var paths: Paths? = nil
    struct Paths: Decodable, Equatable {
        let frame: String
        let items: [Stroke]
    }
    struct Stroke: Decodable, Equatable {
        let id: String
        let title: String
        let color: String
        let width_m: Float
        let segments: [[[Float]]]
    }
    /// Where Home places this scene on the Earth, once any phone has told it.
    var geo: HomeGeo? = nil
    let local_mapping: Mapping?
    struct Voxel: Decodable, Equatable {
        let center: [Float]
        let linear_rgb: [Float]?
    }
    struct ObjectLabel: Decodable, Equatable {
        let id: String
        let label: String
        let position: [Float]
    }
    struct Note: Decodable, Equatable {
        let id: String
        let text: String
        let position: [Float]
        /// Present on Home's phone-presence labels: that phone's camera pose.
        let phone_pose: PhonePose?
        /// Present on Home's robot labels: the robot's base pose. Unlike a phone's,
        /// the base frame is Z up with +X forward.
        var robot_pose: PhonePose? = nil
    }
    struct PhonePose: Decodable, Equatable {
        let peer_id: String
        let name: String?
        let state: String?
        let pose: Portal.Pose
    }
    struct Mapping: Decodable, Equatable {
        let robot_pose: RobotPose?
        let runners: [String: Runner]?
    }
    struct Runner: Decodable, Equatable {
        let robot_pose: RobotPose?
        let status: String?
        let detail: String?
    }
    struct RobotPose: Decodable, Equatable {
        let frame: String
        let pose: RobotTransform
    }
    struct RobotTransform: Decodable, Equatable {
        let translation: [Float]
        let rotation_xyzw: [Float]
        var matrix: simd_float4x4 {
            let q = rotation_xyzw
            var result = simd_float4x4(simd_quatf(ix: q[0], iy: q[1], iz: q[2], r: q[3]))
            result.columns.3 = SIMD4(translation[0], translation[1], translation[2], 1)
            return result
        }
    }
}

struct CalibrationCooldown {
    private var acceptedAt: TimeInterval?
    func canScan(at time: TimeInterval) -> Bool {
        acceptedAt.map { time - $0 >= 1 } ?? true
    }
    mutating func accept(at time: TimeInterval) { acceptedAt = time }
    mutating func reset() { acceptedAt = nil }
}

struct NotePlacement: Identifiable, Encodable {
    let id: String
    let frame: String
    let origin: [Float]
    let direction: [Float]
    var text = ""
    /// Set when the note goes exactly here, with no surface for Home to find along the ray.
    var at: [Float]? = nil
    private enum CodingKeys: String, CodingKey { case id, frame, origin, direction, text }
}

enum SceneRay {
    static func hit(origin: SIMD3<Float>, direction: SIMD3<Float>, scene: RenderScene?) -> SIMD3<Float>? {
        guard origin.x.isFinite, origin.y.isFinite, origin.z.isFinite,
              simd_length(direction).isFinite, simd_length(direction) > 0.000001 else { return nil }
        let d = simd_normalize(direction)
        var nearest: Float = 40.0001
        if abs(d.y) > 0.00000001 {
            let t = -origin.y / d.y
            if t > 0.001 && t <= 40 { nearest = t }
        }
        if let scene, scene.voxel_size > 0 {
            let half = scene.voxel_size / 2
            for voxel in scene.voxels {
                guard voxel.center.count == 3 else { continue }
                var near: Float = 0
                var far: Float = 40
                for axis in 0..<3 {
                    let low = voxel.center[axis] - half, high = voxel.center[axis] + half
                    if abs(d[axis]) < 0.00000001 {
                        if origin[axis] < low || origin[axis] > high { far = -1; break }
                    } else {
                        let a = (low - origin[axis]) / d[axis], b = (high - origin[axis]) / d[axis]
                        near = max(near, min(a,b)); far = min(far, max(a,b))
                    }
                }
                if near <= far && near < nearest { nearest = near }
            }
        }
        return nearest <= 40 ? origin + d * nearest : nil
    }
}

struct PhonePoseSample: Encodable {
    let session: String
    let timestamp_ns: UInt64
    let pose: Portal.Pose
    init(session: String, timestamp: TimeInterval, cameraInAR: simd_float4x4,
         homeInAR: simd_float4x4, homeFrame: String) {
        self.session = session
        timestamp_ns = UInt64(timestamp * 1_000_000_000)
        let cameraInHome = homeInAR.inverse * cameraInAR
        let q = simd_normalize(simd_quatf(cameraInHome))
        pose = Portal.Pose(from_frame_id: "phone-camera:\(session)", to_frame_id: homeFrame,
            translation: [cameraInHome.columns.3.x, cameraInHome.columns.3.y, cameraInHome.columns.3.z],
            rotation_wxyz: [q.real, q.imag.x, q.imag.y, q.imag.z])
    }
}

extension Portal.Pose {
    static func transform(_ matrix: simd_float4x4, from: String, to: String) -> Self {
        let q = simd_normalize(simd_quatf(matrix))
        return Self(from_frame_id: from, to_frame_id: to,
                    translation: [matrix.columns.3.x, matrix.columns.3.y, matrix.columns.3.z],
                    rotation_wxyz: [q.real, q.imag.x, q.imag.y, q.imag.z])
    }
}
struct DepthAlignment: Encodable {
    let timestamp_ns: UInt64
    let portal_id: String
    let portal_to_session: Portal.Pose
    let session_to_home: Portal.Pose
}
/// A Portal this scene does not know yet, measured in the phone's AR session.
/// Home validates `alignment` and composes the Portal's scene pose itself.
struct PortalReport: Encodable {
    static let assumedSide: Float = 0.15
    let session: String
    let timestamp_ns: UInt64
    let code: String
    let side_length_m: Float
    let portal_to_session: Portal.Pose
    /// Left out for the first Portal of a scene that has none; Home makes it the origin.
    let alignment: DepthAlignment?
    /// True moves a Portal the scene already has; left out, the Portal is new.
    var replace: Bool? = nil
}
struct PhoneDepthSample: Encodable {
    let session: String
    let timestamp_ns: UInt64
    let camera_to_session: Portal.Pose
    let alignment: DepthAlignment
    let points: [[Float]]
    /// sRGB camera color per point; omitted from the JSON when nil.
    var colors: [[UInt8]]? = nil
}
/// The phone's own map: every 10 cm cell this AR session has measured, in the
/// session's own frame. Home shows it by itself and never merges it.
struct PhoneLocalMap: Encodable {
    let session: String
    let timestamp_ns: UInt64
    let cells: [[Int32]]
}
struct PhoneLocalVoxels {
    /// Home accepts at most this many cells, each within this many cells of the origin.
    static let limit = 10_000, reach: Int32 = 10_000
    private var seen = Set<SIMD3<Int32>>()
    /// In the order first measured. A full map takes no new cells.
    private(set) var cells = [[Int32]]()
    mutating func add(_ cell: SIMD3<Int32>) {
        guard cells.count < Self.limit, all(abs(cell) .<= SIMD3(repeating: Self.reach)),
              seen.insert(cell).inserted else { return }
        cells.append([cell.x, cell.y, cell.z])
    }
}
enum DepthProjection {
    // ARKit's depth image uses camera-image coordinates. Rescale intrinsics to
    // that resolution, then express measured points in camera OpenGL axes.
    static func point(x: Int, y: Int, depth: Float, width: Int, height: Int,
                      imageWidth: Int, imageHeight: Int, intrinsics: simd_float3x3) -> [Float]? {
        guard depth.isFinite, depth >= 0.2, depth <= 5, width > 0, height > 0,
              imageWidth > 0, imageHeight > 0 else { return nil }
        let sx = Float(width) / Float(imageWidth), sy = Float(height) / Float(imageHeight)
        let fx = intrinsics[0][0] * sx, fy = intrinsics[1][1] * sy
        guard fx > 0, fy > 0 else { return nil }
        let px = (Float(x) - intrinsics[2][0] * sx) * depth / fx
        let py = -(Float(y) - intrinsics[2][1] * sy) * depth / fy
        guard px.isFinite, py.isFinite, px*px + py*py + depth*depth <= 64 else { return nil }
        return [px, py, -depth]
    }
}

/// A phone or robot placed in Home's frame, with the path it has taken while this phone watched.
struct TrackedDevice: Equatable, Identifiable {
    enum Kind: Equatable { case phone, robot }
    /// `asleep` is a phone that says so; `lost` is anything Home has stopped reporting.
    enum State: Equatable { case live, asleep, lost }
    /// "phone:" or "robot:" followed by the peer ID, as in the device lists.
    let id: String
    let kind: Kind
    let name: String
    /// A phone's camera pose, or a robot's base pose (Z up, +X forward), in Home's frame.
    let pose: simd_float4x4
    let trail: [SIMD3<Float>]
    var state = State.live
    /// When Home last reported it.
    var lastSeen = Date()
    var position: SIMD3<Float> { SIMD3(pose.columns.3.x, pose.columns.3.y, pose.columns.3.z) }
    /// Metres along the recorded path.
    var travelled: Float { zip(trail, trail.dropFirst()).reduce(0) { $0 + simd_distance($1.0, $1.1) } }
}

/// Home sends where each device is now, not where it has been. The phone keeps
/// the path, and the last known place of anything Home stops reporting.
struct DeviceTrails {
    /// A new point is kept once the device has moved this far, so one at rest adds nothing.
    static let step: Float = 0.05
    static let limit = 3000
    /// Home republishes about once a second and sometimes skips a device for a beat.
    /// Only this long without a report makes it lost.
    static let patience: TimeInterval = 8
    private(set) var frame: String?
    private(set) var paths = [String: [SIMD3<Float>]]()
    private var seen = [String: TrackedDevice]()
    /// This phone's own ID, learned from Home's entry for its camera frame.
    private var own: String?

    /// Forget the paths, not the devices.
    mutating func clear() {
        paths = [:]
        seen = seen.mapValues {
            TrackedDevice(id: $0.id, kind: $0.kind, name: $0.name, pose: $0.pose, trail: [], state: $0.state, lastSeen: $0.lastSeen)
        }
    }

    /// Trail colour as sRGB: pale blue where the device was first, yellow where it is now.
    static func color(age fraction: Float) -> SIMD3<Float> {
        let t = max(0, min(1, fraction))
        return simd_mix(SIMD3<Float>(0.62, 0.82, 1.0), SIMD3<Float>(1.0, 0.86, 0.25), SIMD3(repeating: t))
    }

    /// Record every phone and robot Home places in `frame`, and return each device
    /// seen so far with its trail. `ownFrame` is this phone's own camera frame.
    mutating func record(_ notes: [RenderScene.Note], frame: String, ownFrame: String? = nil,
                         now: Date = Date()) -> [TrackedDevice] {
        // Positions from another frame cannot share a path.
        if self.frame != frame { paths = [:]; seen = [:]; self.frame = frame }
        // Home may still hold this phone's previous session for a moment after a
        // relaunch. It is this phone, not another one that then went missing.
        if let mine = notes.compactMap(\.phone_pose).first(where: { $0.pose.from_frame_id == ownFrame }) {
            own = "phone:" + mine.peer_id
            seen[own!] = nil; paths[own!] = nil
        }
        var reported = Set<String>()
        for note in notes {
            let kind: TrackedDevice.Kind, sample: RenderScene.PhonePose
            if let robot = note.robot_pose { (kind, sample) = (.robot, robot) }
            else if let phone = note.phone_pose, phone.pose.from_frame_id != ownFrame { (kind, sample) = (.phone, phone) }
            else { continue }
            guard sample.pose.to_frame_id == frame,
                  sample.pose.translation.count == 3, sample.pose.rotation_wxyz.count == 4,
                  (sample.pose.translation + sample.pose.rotation_wxyz).allSatisfy(\.isFinite) else { continue }
            let id = (kind == .robot ? "robot:" : "phone:") + sample.peer_id
            if id == own { continue }
            let pose = sample.pose.matrix
            let at = SIMD3(pose.columns.3.x, pose.columns.3.y, pose.columns.3.z)
            var path = paths[id] ?? []
            if path.last.map({ simd_distance($0, at) >= Self.step }) ?? true {
                path.append(at)
                if path.count > Self.limit { path.removeFirst(path.count - Self.limit) }
                paths[id] = path
            }
            seen[id] = TrackedDevice(id: id, kind: kind, name: sample.name ?? note.text, pose: pose, trail: path,
                                     state: sample.state == "asleep" ? .asleep : .live, lastSeen: now)
            reported.insert(id)
        }
        for (id, device) in seen where !reported.contains(id) && now.timeIntervalSince(device.lastSeen) > Self.patience {
            seen[id]?.state = .lost
        }
        return seen.values.sorted { $0.id < $1.id }
    }
}
