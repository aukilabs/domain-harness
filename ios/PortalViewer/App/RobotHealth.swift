import Foundation
import simd

/// Motor health Home relays with robot presence: per-joint temperature and
/// current keyed by URDF joint name, and the battery. Presentation only.
struct RobotHealth: Decodable, Equatable {
    let timestamp_ns: UInt64
    var joints: [String: Joint]? = nil
    var battery: Battery? = nil
    struct Joint: Decodable, Equatable {
        let temperature_c: Double
        let current_a: Double
    }
    struct Battery: Decodable, Equatable {
        let voltage: Double
        let percent: Double
    }

    /// The hottest joint, if any is warm enough to call out.
    var warning: (joint: String, celsius: Double, level: Level)? {
        guard let (name, joint) = joints?.max(by: { $0.value.temperature_c < $1.value.temperature_c }) else { return nil }
        let level = Self.level(celsius: joint.temperature_c)
        return level == .ok ? nil : (name, joint.temperature_c, level)
    }

    enum Level: Equatable { case ok, warm, hot }

    // Same thresholds as the Home viewer (web/robot-health.js). BBOS drops a
    // motor's torque after 15 s above 50 C (sustained_temp_limit).
    static let coolC = 35.0, warmC = 42.0, limitC = 50.0

    static func level(celsius: Double) -> Level {
        celsius >= limitC ? .hot : celsius > warmC ? .warm : .ok
    }
    static func level(batteryPercent percent: Double) -> Level {
        percent >= 50 ? .ok : percent >= 20 ? .warm : .hot
    }

    /// Temperature colour as RGB 0...1: green to amber over 35-42 C, amber to red over 42-50 C.
    static func color(celsius: Double) -> SIMD3<Double> {
        let cool = SIMD3(0.133, 0.773, 0.369), warm = SIMD3(0.961, 0.620, 0.043), hot = SIMD3(0.937, 0.267, 0.267)
        if celsius <= coolC { return cool }
        if celsius <= warmC { return mix(cool, warm, t: (celsius - coolC) / (warmC - coolC)) }
        return mix(warm, hot, t: min(1, (celsius - warmC) / (limitC - warmC)))
    }
}

/// Joint angles Home relays with robot presence, keyed by URDF joint name.
struct RobotJoints: Decodable, Equatable {
    let timestamp_ns: UInt64
    let positions: [String: Double]
}

/// The Bracketbot URDF's kinematic tree, for placing things on its joints.
/// Mirrors web/robot-model.js: fixed, revolute, continuous and prismatic joints
/// plus <mimic>. Frames are in the URDF root link's ROS body axes (+X forward,
/// +Y left, +Z up), in metres.
struct RobotKinematics {
    struct Joint {
        let name: String
        let type: String
        let parent: String
        let child: String
        let origin: simd_double4x4
        let axis: SIMD3<Double>
        let lower: Double
        let upper: Double
        let mimic: (joint: String, multiplier: Double, offset: Double)?
    }
    /// Joints ordered so each one's parent link is placed before it.
    let joints: [Joint]
    let root: String

    /// Each movable joint's moving frame for these angles: its position is where
    /// the joint is. A joint without a value is at zero, as in the Home viewer.
    func frames(_ positions: [String: Double]) -> [String: simd_double4x4] {
        var links = [root: matrix_identity_double4x4]
        var frames = [String: simd_double4x4]()
        for joint in joints {
            guard let parent = links[joint.parent] else { continue }
            let value = joint.mimic.map { (positions[$0.joint] ?? 0) * $0.multiplier + $0.offset } ?? positions[joint.name] ?? 0
            let clamped = joint.type == "continuous" ? value : min(joint.upper, max(joint.lower, value))
            var motion = matrix_identity_double4x4
            switch joint.type {
            case "prismatic": motion.columns.3 = SIMD4(joint.axis * clamped, 1)
            case "revolute", "continuous": motion = simd_double4x4(simd_quatd(angle: clamped, axis: joint.axis))
            default: break
            }
            let frame = parent * joint.origin * motion
            links[joint.child] = frame
            if joint.type != "fixed" { frames[joint.name] = frame }
        }
        return frames
    }

    /// Parse a URDF. Throws if it has no single root link or a joint is malformed.
    static func load(_ data: Data) throws -> RobotKinematics {
        let reader = Reader()
        let parser = XMLParser(data: data)
        parser.delegate = reader
        guard parser.parse() else { throw ViewerError("Robot URDF is not valid XML") }
        let children = Set(reader.joints.map(\.child))
        let roots = reader.links.filter { !children.contains($0) }
        guard roots.count == 1 else { throw ViewerError("Robot URDF needs one root link, found \(roots.count)") }
        // Breadth-first from the root, so parents come first.
        var ordered = [Joint](), placed: Set<String> = [roots[0]], remaining = reader.joints
        while true {
            let ready = remaining.filter { placed.contains($0.parent) }
            if ready.isEmpty { break }
            ordered += ready
            ready.forEach { placed.insert($0.child) }
            remaining.removeAll { placed.contains($0.child) }
        }
        return RobotKinematics(joints: ordered, root: roots[0])
    }

    /// URDF origin: translation `xyz`, then fixed-axis roll, pitch, yaw: R = Rz(yaw) Ry(pitch) Rx(roll).
    static func origin(xyz: SIMD3<Double>, rpy: SIMD3<Double>) -> simd_double4x4 {
        let rotation = simd_quatd(angle: rpy.z, axis: SIMD3(0, 0, 1))
            * simd_quatd(angle: rpy.y, axis: SIMD3(0, 1, 0))
            * simd_quatd(angle: rpy.x, axis: SIMD3(1, 0, 0))
        var m = simd_double4x4(rotation)
        m.columns.3 = SIMD4(xyz, 1)
        return m
    }

    private final class Reader: NSObject, XMLParserDelegate {
        var links = [String]()
        var joints = [Joint]()
        private var stack = [String]()
        private var current: [String: [String: String]]?
        private var name = "", type = ""

        func parser(_ parser: XMLParser, didStartElement element: String, namespaceURI: String?,
                    qualifiedName: String?, attributes: [String: String] = [:]) {
            let parent = stack.last
            stack.append(element)
            if element == "link", parent == "robot", let name = attributes["name"] { links.append(name) }
            if element == "joint", parent == "robot" {
                name = attributes["name"] ?? ""; type = attributes["type"] ?? "fixed"; current = [:]
            } else if parent == "joint", stack.count == 3 {
                // Only a joint's own children: links also contain <origin>s.
                current?[element] = attributes
            }
        }

        func parser(_ parser: XMLParser, didEndElement element: String, namespaceURI: String?, qualifiedName: String?) {
            stack.removeLast()
            guard element == "joint", stack.last == "robot", let parts = current else { return }
            current = nil
            guard let parentLink = parts["parent"]?["link"], let childLink = parts["child"]?["link"] else { return }
            let mimic = parts["mimic"].flatMap { m in m["joint"].map {
                ($0, Double(m["multiplier"] ?? "") ?? 1, Double(m["offset"] ?? "") ?? 0) } }
            let axis = Self.vector(parts["axis"]?["xyz"], fallback: SIMD3(1, 0, 0))
            joints.append(Joint(
                name: name, type: type, parent: parentLink, child: childLink,
                origin: RobotKinematics.origin(xyz: Self.vector(parts["origin"]?["xyz"], fallback: .zero),
                                               rpy: Self.vector(parts["origin"]?["rpy"], fallback: .zero)),
                axis: simd_length(axis) > 0 ? simd_normalize(axis) : SIMD3(1, 0, 0),
                lower: Double(parts["limit"]?["lower"] ?? "") ?? -.infinity,
                upper: Double(parts["limit"]?["upper"] ?? "") ?? .infinity,
                mimic: mimic))
        }

        private static func vector(_ text: String?, fallback: SIMD3<Double>) -> SIMD3<Double> {
            let values = (text ?? "").split(whereSeparator: \.isWhitespace).compactMap { Double($0) }
            return values.count == 3 && values.allSatisfy(\.isFinite) ? SIMD3(values[0], values[1], values[2]) : fallback
        }
    }
}

private func mix(_ a: SIMD3<Double>, _ b: SIMD3<Double>, t: Double) -> SIMD3<Double> { a + (b - a) * t }
