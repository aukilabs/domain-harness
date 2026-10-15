import Foundation

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
}
