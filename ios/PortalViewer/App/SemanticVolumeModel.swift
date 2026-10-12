import Foundation

/// Bottom centre, local X/Y/Z dimensions, yaw about +Y in Home metres.
struct SemanticVolume: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var semantic_type: String
    var position: [Float]
    var dimensions: [Float]
    var yaw: Float
    /// Home's free-text notes for the region. Kept so an edit does not drop it.
    var description: String? = nil
    /// The thinnest volume Home accepts. One this thin is a surface, not a box.
    static let flatHeight: Float = 0.05
    /// Surfaces of this type are drawn as a circle; any other type is a rectangle.
    static let circleType = "circle"
    /// Lying on a floor or a table.
    var flat: Bool { dimensions.count == 3 && dimensions[1] <= Self.flatHeight }
    /// Standing against a wall or a screen, facing along its local +Z.
    var upright: Bool { dimensions.count == 3 && dimensions[2] <= Self.flatHeight && !flat }
    var surface: Bool { flat || upright }
    var valid: Bool {
        UUID(uuidString: id) != nil && position.count == 3 && dimensions.count == 3
        && position.allSatisfy { $0.isFinite && abs($0) < 1_000_000 }
        && dimensions.allSatisfy { $0.isFinite && (0.05...40).contains($0) }
        && yaw.isFinite && abs(yaw) <= .pi
        && !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && name.count <= 120
        && !semantic_type.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && semantic_type.count <= 80
        && !name.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        && !semantic_type.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
    }
}
