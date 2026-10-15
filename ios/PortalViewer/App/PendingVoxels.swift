import Foundation
import simd

/// Local scan groups retire after two subsequent Home map updates. This is a
/// visual progress heuristic, not an acknowledgement that Home accepted a scan.
struct PendingVoxels {
    static let voxelSize: Float = 0.1
    static let limit = 4096
    private(set) var frame = ""
    private(set) var session = ""
    private var generation: UInt64 = 0
    private var groups: [(expires: UInt64, cells: Set<SIMD3<Int32>>)] = []
    var cells: [SIMD3<Int32>: UInt64] {
        var result = [SIMD3<Int32>: UInt64]()
        for group in groups { for cell in group.cells { result[cell] = group.expires } }
        return result
    }

    mutating func clear() { self = Self() }

    mutating func capture(points: [[Float]], cameraToHome: simd_float4x4,
                          frame: String, session: String) {
        guard !frame.isEmpty, !session.isEmpty else { return }
        if self.frame != frame || self.session != session { clear() }
        self.frame = frame; self.session = session
        var captured = Set<SIMD3<Int32>>()
        for point in points where point.count == 3 {
            let p = cameraToHome * SIMD4(point[0], point[1], point[2], 1)
            let grid = SIMD3(p.x, p.y, p.z) / Self.voxelSize
            // Keep conversion bounded even if a malformed transform reaches the UI.
            guard grid.x.isFinite, grid.y.isFinite, grid.z.isFinite,
                  abs(grid.x) < 1_000_000, abs(grid.y) < 1_000_000, abs(grid.z) < 1_000_000 else { continue }
            let cell = SIMD3(Int32(floor(grid.x)), Int32(floor(grid.y)), Int32(floor(grid.z)))
            captured.insert(cell)
            if captured.count == Self.limit { break }
        }
        guard !captured.isEmpty else { return }
        groups.append((generation + 2, captured))
        // Bound all retained groups, including overlapping scans, during a stall.
        while groups.reduce(0, { $0 + $1.cells.count }) > Self.limit {
            groups.removeFirst()
        }
    }

    mutating func receivedMapUpdate(frame: String) -> Set<SIMD3<Int32>> {
        guard frame == self.frame else { return [] }
        let before = Set(cells.keys)
        generation += 1
        groups.removeAll { $0.expires <= generation }
        // A newer scan can still be waiting at the same location.
        return before.subtracting(cells.keys)
    }
}
