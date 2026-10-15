import SceneKit
import simd

/// One batched mesh, bounded independently of Home's map size. Small magenta
/// wireframe cubes leave space for the full-size confirmed voxels that replace them.
final class PendingVoxelOverlay {
    let root: SCNNode = { let node = SCNNode(); node.name = "voxels"; return node }()
    private let material: SCNMaterial = {
        let material = SCNMaterial()
        material.lightingModel = .constant
        material.diffuse.contents = CGColor(red: 1, green: 0.12, blue: 0.72, alpha: 0.8)
        material.blendMode = .alpha
        material.writesToDepthBuffer = false
        return material
    }()
    private var displayed = Set<SIMD3<Int32>>()
    private var frame = "", session = ""
    private var fading = [SCNNode]()
    func clear() {
        displayed.removeAll(); root.geometry = nil
        for node in fading { node.removeAllActions(); node.removeFromParentNode() }
        fading.removeAll(); frame = ""; session = ""
    }

    func update(_ pending: PendingVoxels, retired: Set<SIMD3<Int32>> = []) {
        if pending.frame != frame || pending.session != session { clear() }
        frame = pending.frame; session = pending.session
        let cells = Set(pending.cells.keys)
        guard cells != displayed else { return }
        // Only groups retired by map progress fade; eviction/reset does not.
        let returned = displayed.subtracting(cells).intersection(retired)
        fading.removeAll { $0.parent == nil }
        if !returned.isEmpty {
            // Bound transient geometry even when many updates arrive together.
            if fading.count == 4 {
                let oldest = fading.removeFirst()
                oldest.removeAllActions(); oldest.removeFromParentNode()
            }
            let node = SCNNode(geometry: mesh(returned))
            let fadeMaterial = material.copy() as! SCNMaterial
            // The small outline is inside the newly returned solid voxel.
            // Keep its brief fade visible over that voxel, then remove it.
            fadeMaterial.readsFromDepthBuffer = false
            node.geometry?.materials = [fadeMaterial]
            node.renderingOrder = 10
            root.addChildNode(node); fading.append(node)
            node.runAction(.sequence([.fadeOut(duration: 0.45), .removeFromParentNode()]))
        }
        displayed = cells
        root.geometry = cells.isEmpty ? nil : mesh(cells)
    }

    private func mesh(_ cells: Set<SIMD3<Int32>>) -> SCNGeometry {
        let corners: [SIMD3<Float>] = [
            SIMD3(-1,-1,-1), SIMD3(1,-1,-1), SIMD3(1,1,-1), SIMD3(-1,1,-1),
            SIMD3(-1,-1,1), SIMD3(1,-1,1), SIMD3(1,1,1), SIMD3(-1,1,1)
        ]
        let edges: [UInt32] = [0,1, 1,2, 2,3, 3,0, 4,5, 5,6, 6,7, 7,4, 0,4, 1,5, 2,6, 3,7]
        let halfSide = PendingVoxels.voxelSize * 0.225
        var vertices = [SCNVector3](); vertices.reserveCapacity(cells.count * 8)
        var indices = [UInt32](); indices.reserveCapacity(cells.count * 24)
        for cell in cells {
            let center = (SIMD3<Float>(cell) + 0.5) * PendingVoxels.voxelSize
            let first = UInt32(vertices.count)
            for corner in corners {
                let p = center + corner * halfSide
                vertices.append(SCNVector3(p.x, p.y, p.z))
            }
            indices.append(contentsOf: edges.map { first + $0 })
        }
        let mesh = SCNGeometry(sources: [SCNGeometrySource(vertices: vertices)],
                              elements: [SCNGeometryElement(indices: indices, primitiveType: .line)])
        mesh.materials = [material]
        return mesh
    }
}
