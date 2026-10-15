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
    var render: RenderScene?
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
    var voxels: [Voxel]
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

/// A path drawn on the scene with a finger, as Home's Crayon saves it: metres in Home's frame.
struct CrayonStroke: Encodable, Equatable {
    static let color = "#ffca55"
    static let width: Float = 0.035
    /// The widths Home accepts.
    static let widths: ClosedRange<Float> = 0.005...0.25
    let id: String
    let title: String
    let color: String
    let width_m: Float
    let segments: [[[Float]]]
}
/// A stroke while it is being drawn. Home's rules: points at least 2 cm apart, at most
/// 4096 of them, and a miss ends the segment so the line never jumps across a gap.
struct CrayonDraft {
    static let spacing: Float = 0.02, limit = 4096
    private(set) var segments = [[SIMD3<Float>]]()
    private var open = false
    /// Takes the surface point under the finger, or nil where there was none.
    /// Returns the piece of line this added, if any.
    mutating func add(_ point: SIMD3<Float>?) -> (SIMD3<Float>, SIMD3<Float>)? {
        guard let point else { open = false; return nil }
        guard segments.reduce(0, { $0 + $1.count }) < Self.limit else { return nil }
        guard open, let last = segments.last?.last else {
            segments.append([point]); open = true; return nil
        }
        guard simd_distance(last, point) >= Self.spacing else { return nil }
        segments[segments.count - 1].append(point)
        return (last, point)
    }
    /// Nothing to save until some segment has two points.
    func stroke(id: String, color: String = CrayonStroke.color, width: Float = CrayonStroke.width) -> CrayonStroke? {
        let drawn = segments.filter { $0.count >= 2 }
        guard !drawn.isEmpty else { return nil }
        return CrayonStroke(id: id, title: "Crayon path", color: color, width_m: width,
                            segments: drawn.map { $0.map { [$0.x, $0.y, $0.z] } })
    }
}
/// A drawn line as one smooth tube: a curve through the sampled points, not a chain of straight rods.
enum CrayonTube {
    /// A Catmull-Rom curve through every point, with `steps` pieces between each pair.
    static func smooth(_ points: [SIMD3<Float>], steps: Int = 4) -> [SIMD3<Float>] {
        guard points.count > 2, steps > 1 else { return points }
        var curve = [SIMD3<Float>]()
        for i in 0..<(points.count - 1) {
            let a = points[max(i - 1, 0)], b = points[i], c = points[i + 1], d = points[min(i + 2, points.count - 1)]
            for step in 0..<steps {
                let t = Float(step) / Float(steps), t2 = t * t, t3 = t2 * t
                let p0 = 2 * b, p1 = (c - a) * t
                let p2 = (2 * a - 5 * b + 4 * c - d) * t2
                let p3 = (3 * b - a - 3 * c + d) * t3
                curve.append((p0 + p1 + p2 + p3) * 0.5)
            }
        }
        curve.append(points[points.count - 1])
        return curve
    }
    /// Rings of `sides` vertices round the curve, joined into one closed skin with flat ends.
    /// Each vertex's `along` is its distance in metres from the first point, the way the line was drawn.
    static func mesh(_ points: [SIMD3<Float>], radius: Float, sides: Int = 8) -> (vertices: [SIMD3<Float>], indices: [UInt32], along: [SIMD2<Float>])? {
        var path = [SIMD3<Float>]()
        for p in smooth(points) where p.x.isFinite && p.y.isFinite && p.z.isFinite {
            if let last = path.last, simd_distance(last, p) < 0.0005 { continue }
            path.append(p)
        }
        guard path.count >= 2, radius > 0, sides >= 3 else { return nil }
        var vertices = [SIMD3<Float>](), indices = [UInt32](), along = [SIMD2<Float>]()
        var distance: Float = 0
        // The ring's "up" is carried along the curve so the tube never twists or pinches.
        var normal = SIMD3<Float>(0, 1, 0)
        for i in path.indices {
            let tangent = simd_normalize(path[min(i + 1, path.count - 1)] - path[max(i - 1, 0)])
            var side = normal - tangent * simd_dot(normal, tangent)
            if simd_length(side) < 0.001 {
                let other: SIMD3<Float> = abs(tangent.x) < 0.9 ? SIMD3(1, 0, 0) : SIMD3(0, 0, 1)
                side = other - tangent * simd_dot(other, tangent)
            }
            normal = simd_normalize(side)
            let across = simd_cross(tangent, normal)
            if i > 0 { distance += simd_distance(path[i - 1], path[i]) }
            for k in 0..<sides {
                let angle = Float(k) / Float(sides) * 2 * .pi
                vertices.append(path[i] + (normal * cos(angle) + across * sin(angle)) * radius)
                along.append(SIMD2(distance, Float(k) / Float(sides)))
            }
        }
        let n = UInt32(sides)
        for i in 0..<UInt32(path.count - 1) {
            for k in 0..<n {
                let a = i * n + k, b = i * n + (k + 1) % n, c = a + n, d = b + n
                indices.append(contentsOf: [a, b, c, b, d, c])
            }
        }
        let first = UInt32(vertices.count); vertices.append(path[0]); along.append(SIMD2(0, 0))
        let last = first + 1; vertices.append(path[path.count - 1]); along.append(SIMD2(distance, 0))
        let end = UInt32(path.count - 1) * n
        for k in 0..<n {
            indices.append(contentsOf: [first, (k + 1) % n, k])
            indices.append(contentsOf: [last, end + k, end + (k + 1) % n])
        }
        return (vertices, indices, along)
    }
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
    init(session: String, timestamp: TimeInterval, cameraInAR: simd_float4x4) {
        self.session = session
        timestamp_ns = UInt64(timestamp * 1_000_000_000)
        let q = simd_normalize(simd_quatf(cameraInAR))
        pose = Portal.Pose(from_frame_id: "phone-camera:\(session)", to_frame_id: "phone-ar:\(session)",
            translation: [cameraInAR.columns.3.x, cameraInAR.columns.3.y, cameraInAR.columns.3.z],
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
/// Where the phone's AR session sits in the scene, published for any peer to read
/// whether or not depth is being shared.
struct PhonePlacement: Encodable {
    let session: String
    let timestamp_ns: UInt64
    let session_to_scene: Portal.Pose
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

/// People's notes as messages, in Home's order from first to last, and which have not been opened on this phone.
struct MessageInbox: Equatable {
    struct Message: Identifiable, Equatable {
        let id: String
        var text: String
        var position: SIMD3<Float>
        var unread = false
    }
    private(set) var messages = [Message]()
    private var frame: String?
    var unread: Int { messages.filter(\.unread).count }

    /// Take Home's latest notes and return the ones that are news. What a scene already holds
    /// when it is first seen is not news, and neither is a note this phone wrote.
    mutating func record(_ notes: [RenderScene.Note], frame: String, mine: Set<String> = []) -> [Message] {
        let first = self.frame != frame
        let before = first ? [:] : Dictionary(messages.map { ($0.id, $0.unread) }, uniquingKeysWith: { a, _ in a })
        self.frame = frame
        var arrived = [Message](), listed = Set<String>()
        messages = notes.compactMap { note in
            // Home's own labels for phones and robots are not messages.
            guard note.phone_pose == nil, note.robot_pose == nil, note.position.count == 3,
                  note.position.allSatisfy(\.isFinite), listed.insert(note.id).inserted else { return nil }
            let message = Message(id: note.id, text: note.text,
                                  position: SIMD3(note.position[0], note.position[1], note.position[2]),
                                  unread: before[note.id] ?? (!first && !mine.contains(note.id)))
            if before[note.id] == nil && message.unread { arrived.append(message) }
            return message
        }
        return arrived
    }
    mutating func open(_ id: String) {
        if let index = messages.firstIndex(where: { $0.id == id }) { messages[index].unread = false }
    }
    /// Two messages to show the way between, from the first to the second.
    struct Pair: Equatable {
        var from: Message
        var to: Message
    }
    /// The same two as Home has them now. Nil once either is gone.
    func current(_ pair: Pair) -> Pair? {
        guard let from = messages.first(where: { $0.id == pair.from.id }),
              let to = messages.first(where: { $0.id == pair.to.id }) else { return nil }
        return Pair(from: from, to: to)
    }
}

/// A way across the floor to a spot, round what the map shows is in the way.
enum MessageRoute {
    /// The floor is Y = 0 in Home's frame. Voxels in this band above it are in a walker's way.
    static let body: ClosedRange<Float> = 0.25...1.9
    /// Farthest a route is planned across, in cells each way.
    static let reach = 320
    static let lift: Float = 0.06

    /// Corners of a walkable way from `from` to `to`, both ends included, just above the floor.
    /// It goes round voxels at body height, keeps a step clear of them where it can, and
    /// prefers floor the map has actually seen to floor it has not. With nothing known in
    /// the way, or no way round, it is the straight line.
    static func plan(from: SIMD3<Float>, to: SIMD3<Float>, voxels: [SIMD3<Float>], size: Float) -> [SIMD3<Float>] {
        let start = SIMD3(from.x, lift, from.z), goal = SIMD3(to.x, lift, to.z)
        guard size > 0.01, [start, goal].allSatisfy({ $0.x.isFinite && $0.z.isFinite }) else { return [start, goal] }
        func cell(_ p: SIMD3<Float>) -> SIMD2<Int> { SIMD2(Int((p.x / size).rounded(.down)), Int((p.z / size).rounded(.down))) }
        let a = cell(start), b = cell(goal)
        let margin = Int((3 / size).rounded(.up))
        let low = SIMD2(min(a.x, b.x) - margin, min(a.y, b.y) - margin)
        let width = abs(a.x - b.x) + 2 * margin + 1, height = abs(a.y - b.y) + 2 * margin + 1
        guard width <= reach, height <= reach else { return [start, goal] }
        func index(_ c: SIMD2<Int>) -> Int? {
            let x = c.x - low.x, y = c.y - low.y
            return x >= 0 && y >= 0 && x < width && y < height ? y * width + x : nil
        }
        // 0 unseen, 1 seen floor, 2 near something, 3 something in the way.
        var grid = [UInt8](repeating: 0, count: width * height)
        var blocked = [SIMD2<Int>]()
        for voxel in voxels {
            guard let i = index(cell(voxel)) else { continue }
            if body.contains(voxel.y) { if grid[i] != 3 { grid[i] = 3; blocked.append(cell(voxel)) } }
            else if voxel.y < body.lowerBound, grid[i] == 0 { grid[i] = 1 }
        }
        guard !blocked.isEmpty else { return [start, goal] }
        let clearance = max(1, Int((0.25 / size).rounded()))
        for c in blocked {
            for dy in -clearance...clearance { for dx in -clearance...clearance {
                if let i = index(SIMD2(c.x + dx, c.y + dy)), grid[i] < 2 { grid[i] = 2 }
            } }
        }
        // Standing in, or heading into, something on the map: those ends stay reachable.
        for end in [a, b] {
            for dy in -clearance...clearance { for dx in -clearance...clearance {
                if let i = index(SIMD2(end.x + dx, end.y + dy)), grid[i] == 3 { grid[i] = 2 }
            } }
        }
        guard let first = index(a), let last = index(b) else { return [start, goal] }
        func price(_ i: Int) -> Float { grid[i] == 1 ? 1 : grid[i] == 0 ? 2.5 : 6 }
        var cost = [Float](repeating: .infinity, count: grid.count), parent = [Int32](repeating: -1, count: grid.count)
        var heap = [(Float, Int)]()
        func push(_ item: (Float, Int)) {
            heap.append(item)
            var child = heap.count - 1
            while child > 0, heap[(child - 1) / 2].0 > heap[child].0 { heap.swapAt(child, (child - 1) / 2); child = (child - 1) / 2 }
        }
        func pop() -> (Float, Int) {
            let top = heap[0], end = heap.removeLast()
            if !heap.isEmpty {
                heap[0] = end
                var node = 0
                while true {
                    var least = node
                    for child in [2 * node + 1, 2 * node + 2] where child < heap.count && heap[child].0 < heap[least].0 { least = child }
                    if least == node { break }
                    heap.swapAt(node, least); node = least
                }
            }
            return top
        }
        func ahead(_ i: Int) -> Float {
            let dx = Float(abs(i % width - last % width)), dy = Float(abs(i / width - last / width))
            return max(dx, dy) + 0.4142 * min(dx, dy)
        }
        cost[first] = 0; push((ahead(first), first))
        while !heap.isEmpty {
            let (rank, here) = pop()
            if here == last { break }
            if rank > cost[here] + ahead(here) + 0.001 { continue }
            let x = here % width, y = here / width
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let nx = x + dx, ny = y + dy
                guard nx >= 0, ny >= 0, nx < width, ny < height else { continue }
                let next = ny * width + nx
                guard grid[next] != 3 else { continue }
                // No squeezing diagonally between two things.
                if dx != 0, dy != 0, grid[y * width + nx] == 3 || grid[ny * width + x] == 3 { continue }
                let reached = cost[here] + (dx != 0 && dy != 0 ? 1.4142 : 1) * price(next)
                if reached < cost[next] { cost[next] = reached; parent[next] = Int32(here); push((reached + ahead(next), next)) }
            }
        }
        guard cost[last].isFinite else { return [start, goal] }
        var cells = [Int](), at = last
        while at != first { cells.append(at); at = Int(parent[at]) }
        cells.append(first); cells.reverse()
        func middle(_ i: Int) -> SIMD3<Float> {
            SIMD3((Float(i % width + low.x) + 0.5) * size, lift, (Float(i / width + low.y) + 0.5) * size)
        }
        var points = [start] + cells.dropFirst().dropLast().map(middle) + [goal]
        // A straight stretch is clear when nothing along it is near a voxel.
        func clear(_ p: SIMD3<Float>, _ q: SIMD3<Float>) -> Bool {
            let steps = max(1, Int((simd_distance(p, q) / (size / 2)).rounded(.up)))
            for step in 1..<max(2, steps) {
                guard let i = index(cell(p + (q - p) * (Float(step) / Float(steps)))), grid[i] < 2 else { return false }
            }
            return true
        }
        var corners = [points[0]], from = 0
        while from < points.count - 1 {
            var next = from + 1
            for far in stride(from: points.count - 1, to: from + 1, by: -1) where clear(points[from], points[far]) { next = far; break }
            corners.append(points[next]); from = next
        }
        points = corners
        return points
    }

    /// Evenly spaced spots along a route, for a trail of crumbs. The last is the route's end.
    static func crumbs(_ route: [SIMD3<Float>], every gap: Float = 0.4) -> [SIMD3<Float>] {
        guard route.count > 1, gap > 0 else { return route }
        var crumbs = [SIMD3<Float>](), owed = gap
        for (a, b) in zip(route, route.dropFirst()) {
            let length = simd_distance(a, b)
            guard length > 0.0001 else { continue }
            var along = owed
            while along <= length { crumbs.append(a + (b - a) * (along / length)); along += gap }
            owed = along - length
        }
        if let end = route.last, crumbs.last.map({ simd_distance($0, end) > gap / 2 }) ?? true { crumbs.append(end) }
        else if let end = route.last { crumbs[crumbs.count - 1] = end }
        return crumbs
    }

    /// How far a spot is from the route, across the floor.
    static func stray(_ spot: SIMD3<Float>, from route: [SIMD3<Float>]) -> Float {
        let p = SIMD2(spot.x, spot.z)
        var nearest = Float.infinity
        for (a3, b3) in zip(route, route.dropFirst()) {
            let a = SIMD2(a3.x, a3.z), b = SIMD2(b3.x, b3.z), along = b - a
            let t = simd_length_squared(along) > 0.000001 ? max(0, min(1, simd_dot(p - a, along) / simd_length_squared(along))) : 0
            nearest = min(nearest, simd_distance(p, a + along * t))
        }
        if route.count == 1, let only = route.first { nearest = simd_distance(p, SIMD2(only.x, only.z)) }
        return nearest
    }

    /// The route as a path for everyone in the scene to see: a line drawn without a crayon.
    /// A way between two messages says where it starts as well.
    static func path(_ route: [SIMD3<Float>], id: String, to name: String, from origin: String? = nil) -> CrayonStroke? {
        guard let start = route.first else { return nil }
        // Closely spaced points keep the smooth line on the route round corners.
        let points = [start] + crumbs(route, every: 0.3)
        guard points.count >= 2, simd_distance(start, points[points.count - 1]) > 0.05 else { return nil }
        let leg = origin.map { "Way from " + $0.trimmingCharacters(in: .whitespacesAndNewlines) + " to " } ?? "Way to "
        let title = String((leg + name.trimmingCharacters(in: .whitespacesAndNewlines)).prefix(120))
        return CrayonStroke(id: id, title: title, color: color, width_m: width, segments: [points.map { [$0.x, $0.y, $0.z] }])
    }
    static let color = "#ff5fa8"
    static let width: Float = 0.05

    /// How far it is along the route from its start to its end.
    static func length(_ route: [SIMD3<Float>]) -> Float {
        zip(route, route.dropFirst()).reduce(0) { $0 + simd_distance($1.0, $1.1) }
    }

    /// How far apart two routes get, across the floor. Small when they are the same way.
    static func apart(_ a: [SIMD3<Float>], _ b: [SIMD3<Float>]) -> Float {
        func off(_ route: [SIMD3<Float>], from other: [SIMD3<Float>]) -> Float {
            (route + crumbs(route, every: 0.3)).map { stray($0, from: other) }.max() ?? .infinity
        }
        return max(off(a, from: b), off(b, from: a))
    }

    /// The spot `distance` along the route from its start: where to head next.
    static func ahead(_ route: [SIMD3<Float>], by distance: Float) -> SIMD3<Float>? {
        guard var spot = route.first else { return nil }
        var left = distance
        for next in route.dropFirst() {
            let length = simd_distance(spot, next)
            if length >= left, length > 0.0001 { return spot + (next - spot) * (left / length) }
            left -= length; spot = next
        }
        return spot
    }
}

/// One end of a route asked of Home: a point in the scene frame, or a note by its ID.
enum RoutePlace: Encodable, Equatable {
    case point(SIMD3<Float>)
    case note(String)
    private enum Key: String, CodingKey { case point, note }
    func encode(to encoder: Encoder) throws {
        var box = encoder.container(keyedBy: Key.self)
        switch self {
        case .point(let p): try box.encode([p.x, p.y, p.z], forKey: .point)
        case .note(let id): try box.encode(id, forKey: .note)
        }
    }
}

/// A route as Home planned it (`spatial-harness.route/v1`). Its path is in the scene's paths
/// like any drawn one, unless it was asked for without sharing.
struct HomeRoute: Decodable, Equatable {
    struct Path: Decodable, Equatable {
        let id: String
        let segments: [[[Float]]]
    }
    let frame: String
    /// `found`, `direct` when nothing known is in the way, or `unreachable`.
    let status: String
    let path: Path?
    let length_m: Float
    /// The way's points from its start to its end. Empty when there is no way.
    var points: [SIMD3<Float>] {
        (path?.segments.first ?? []).compactMap { $0.count == 3 ? SIMD3($0[0], $0[1], $0[2]) : nil }
    }
}

/// Where a spot in the room is from the phone: on the screen, or which way to turn to bring it there.
struct MessageAim: Equatable {
    /// Across and down the screen from its top left corner, each 0 to 1. Nil when the spot is out of view.
    let screen: SIMD2<Float>?
    /// The way to the spot, clockwise from the top of the screen, in radians.
    let turn: Float
    let hint: String
    let distance: Float

    /// `spot` is in the camera's axes as the screen is held: +X right, +Y up, looking along -Z.
    /// `lens` is the projection for that same view.
    init(spot: SIMD3<Float>, lens: simd_float4x4) {
        distance = simd_length(spot)
        let clip = lens * SIMD4(spot.x, spot.y, spot.z, 1)
        let across = clip.x / clip.w, up = clip.y / clip.w
        // A little in from the edges, so the marker is never half off the screen.
        if clip.w > 0, abs(across) < 0.8, abs(up) < 0.8 {
            screen = SIMD2((across + 1) / 2, (1 - up) / 2)
            turn = 0
            hint = "Here"
            return
        }
        screen = nil
        let side = atan2(spot.x, -spot.z), rise = atan2(spot.y, hypot(spot.x, spot.z))
        // Straight behind has no nearer side; turning right gets there.
        turn = hypot(spot.x, spot.y) > 0.0001 ? atan2(spot.x, spot.y) : .pi / 2
        if abs(side) > 2 * .pi / 3 && abs(rise) < .pi / 4 { hint = "Turn around" }
        else if abs(side) >= abs(rise) { hint = side > 0 ? "Turn right" : "Turn left" }
        else { hint = rise > 0 ? "Tilt up" : "Tilt down" }
    }
}
