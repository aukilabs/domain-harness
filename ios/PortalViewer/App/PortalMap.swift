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
    }
    struct Mapping: Decodable, Equatable {
        let robot_pose: RobotPose?
        let runners: [String: Runner]?
    }
    struct Runner: Decodable, Equatable { let robot_pose: RobotPose? }
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
struct PhoneDepthSample: Encodable {
    let session: String
    let timestamp_ns: UInt64
    let camera_to_session: Portal.Pose
    let alignment: DepthAlignment
    let points: [[Float]]
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
