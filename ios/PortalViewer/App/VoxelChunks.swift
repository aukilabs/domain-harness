import SceneKit
import simd

/// Home's voxels drawn the way Minecraft draws blocks: one mesh per 16-cell
/// chunk, and only the faces that border empty space. Two blocks side by side
/// share no face, so there are no hidden faces or edges inside the map.
/// Home sends only the cells that changed; only their chunks are rebuilt.
final class VoxelChunks {
    /// Named "voxels" so the app's voxel toggle hides it like any voxel node.
    let root: SCNNode = { let node = SCNNode(); node.name = "voxels"; return node }()
    private(set) var frame = ""
    private(set) var size: Float = 0
    private var chunkSide: Int32 = 16
    private var cells: [SIMD3<Int32>: SIMD3<UInt8>] = [:]
    private var byChunk: [SIMD3<Int32>: [SIMD3<Int32>]] = [:]
    private var nodes: [SIMD3<Int32>: SCNNode] = [:]
    private let material: SCNMaterial = {
        let m = SCNMaterial()
        m.lightingModel = .constant
        m.blendMode = .alpha
        m.transparencyMode = .aOne
        return m
    }()

    var isEmpty: Bool { cells.isEmpty }

    private struct Update: Decodable {
        let frame: String
        let voxel_size: Float
        let reset: Bool
        let chunk: Int32
        let chunks: [Chunk]
        struct Chunk: Decodable { let key: [Int32]; let cells: [Int32] }
    }

    /// Apply changed chunks from the bridge. True when anything changed.
    @discardableResult
    func apply(_ data: Data) -> Bool {
        guard let update = try? JSONDecoder().decode(Update.self, from: data),
              update.voxel_size.isFinite, update.voxel_size > 0, update.chunk > 0 else { return false }
        if update.reset || update.frame != frame || update.voxel_size != size {
            clear()
            frame = update.frame; size = update.voxel_size; chunkSide = update.chunk
        }
        var dirty = Set<SIMD3<Int32>>()
        for chunk in update.chunks where chunk.key.count == 3 && chunk.cells.count % 6 == 0 {
            let key = SIMD3(chunk.key[0], chunk.key[1], chunk.key[2])
            for old in byChunk[key] ?? [] { cells[old] = nil }
            var list = [SIMD3<Int32>]()
            list.reserveCapacity(chunk.cells.count / 6)
            for i in stride(from: 0, to: chunk.cells.count, by: 6) {
                let c = chunk.cells
                let index = SIMD3(c[i], c[i + 1], c[i + 2])
                cells[index] = SIMD3(UInt8(clamping: c[i + 3]), UInt8(clamping: c[i + 4]), UInt8(clamping: c[i + 5]))
                list.append(index)
            }
            byChunk[key] = list.isEmpty ? nil : list
            dirty.insert(key)
        }
        for key in dirty { rebuild(key) }
        return !dirty.isEmpty
    }

    func clear() {
        cells = [:]; byChunk = [:]
        nodes.values.forEach { $0.removeFromParentNode() }
        nodes = [:]
    }

    /// The occupied cells as the rest of the app reads voxels: centre and linear colour.
    func list() -> [RenderScene.Voxel] {
        cells.map { index, rgb in
            let center = (SIMD3<Float>(index) + 0.5) * size
            return RenderScene.Voxel(center: [center.x, center.y, center.z],
                                     linear_rgb: [Self.linear(rgb.x), Self.linear(rgb.y), Self.linear(rgb.z)])
        }
    }

    private static func linear(_ srgb: UInt8) -> Float {
        let s = Float(srgb) / 255
        return s <= 0.04045 ? s / 12.92 : pow((s + 0.055) / 1.055, 2.4)
    }

    /// The six faces: outward direction, its four corners (counter-clockwise
    /// seen from outside, as unit-cube offsets), and a fixed shade so blocks
    /// read as blocks without lighting.
    private static let faces: [(SIMD3<Int32>, [SIMD3<Float>], Float)] = [
        (SIMD3(1, 0, 0), [SIMD3(1,0,0), SIMD3(1,1,0), SIMD3(1,1,1), SIMD3(1,0,1)], 0.8),
        (SIMD3(-1, 0, 0), [SIMD3(0,0,0), SIMD3(0,0,1), SIMD3(0,1,1), SIMD3(0,1,0)], 0.8),
        (SIMD3(0, 1, 0), [SIMD3(0,1,0), SIMD3(0,1,1), SIMD3(1,1,1), SIMD3(1,1,0)], 1.0),
        (SIMD3(0, -1, 0), [SIMD3(0,0,0), SIMD3(1,0,0), SIMD3(1,0,1), SIMD3(0,0,1)], 0.55),
        (SIMD3(0, 0, 1), [SIMD3(0,0,1), SIMD3(1,0,1), SIMD3(1,1,1), SIMD3(0,1,1)], 0.9),
        (SIMD3(0, 0, -1), [SIMD3(0,0,0), SIMD3(0,1,0), SIMD3(1,1,0), SIMD3(1,0,0)], 0.7),
    ]

    private func rebuild(_ key: SIMD3<Int32>) {
        nodes[key]?.removeFromParentNode()
        nodes[key] = nil
        guard let list = byChunk[key], !list.isEmpty else { return }
        var vertices = [SCNVector3]()
        var colors = [Float]()
        var indices = [UInt32]()
        for index in list {
            guard let rgb = cells[index] else { continue }
            let base = SIMD3<Float>(index) * size
            let color = SIMD3(Self.linear(rgb.x), Self.linear(rgb.y), Self.linear(rgb.z))
            for (direction, corners, shade) in Self.faces where cells[index &+ direction] == nil {
                let first = UInt32(vertices.count)
                for corner in corners {
                    let p = base + corner * size
                    vertices.append(SCNVector3(p.x, p.y, p.z))
                    colors.append(contentsOf: [color.x * shade, color.y * shade, color.z * shade, 0.65])
                }
                indices.append(contentsOf: [first, first + 1, first + 2, first, first + 2, first + 3])
            }
        }
        guard !indices.isEmpty else { return }
        let colorSource = SCNGeometrySource(data: colors.withUnsafeBytes { Data($0) }, semantic: .color,
            vectorCount: vertices.count, usesFloatComponents: true, componentsPerVector: 4,
            bytesPerComponent: 4, dataOffset: 0, dataStride: 16)
        let geometry = SCNGeometry(sources: [SCNGeometrySource(vertices: vertices), colorSource],
                                   elements: [SCNGeometryElement(indices: indices, primitiveType: .triangles)])
        geometry.materials = [material]
        let node = SCNNode(geometry: geometry)
        node.renderingOrder = -1
        root.addChildNode(node)
        nodes[key] = node
    }
}
