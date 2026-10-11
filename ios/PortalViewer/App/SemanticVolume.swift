import Foundation
import SwiftUI
import SceneKit
import ARKit

/// Bottom-centred, upright box in Home's calibrated Y-up metre frame.
extension SemanticVolume {
    func node(preview: Bool = false) -> SCNNode {
        let node = SCNNode()
        node.name = "semantic-volume:" + id
        guard valid else { return node }
        node.simdPosition = SIMD3(position[0], position[1], position[2])
        node.eulerAngles.y = yaw
        let box = SCNBox(width: CGFloat(dimensions[0]), height: CGFloat(dimensions[1]),
                         length: CGFloat(dimensions[2]), chamferRadius: 0)
        let material = SCNMaterial()
        material.diffuse.contents = preview ? UIColor.systemYellow : UIColor.systemCyan
        material.lightingModel = .constant
        material.transparency = 0.25
        material.isDoubleSided = true
        material.writesToDepthBuffer = false
        box.materials = [material]
        let body = SCNNode(geometry: box)
        body.position.y = dimensions[1] / 2
        body.name = node.name
        node.addChildNode(body)
        let edges = SCNNode(geometry: box.copy() as? SCNGeometry)
        let wire = material.copy() as! SCNMaterial
        wire.fillMode = .lines; wire.transparency = 0.9
        edges.geometry?.materials = [wire]; edges.position = body.position
        node.addChildNode(edges)
        let text = SCNText(string: name + " · " + semantic_type, extrusionDepth: 0)
        text.font = UIFont.systemFont(ofSize: 0.12)
        text.flatness = 0.01
        text.firstMaterial?.diffuse.contents = UIColor.white
        text.firstMaterial?.lightingModel = .constant
        let label = SCNNode(geometry: text)
        label.name = node.name; label.position.y = dimensions[1] + 0.05
        label.constraints = [SCNBillboardConstraint()]
        node.addChildNode(label)
        return node
    }
}

/// Native previews update locally; only Done crosses the SDK bridge.
final class VolumeAuthoring: NSObject, ObservableObject, UIGestureRecognizerDelegate {
    @Published var draft: SemanticVolume? { didSet { redraw() } }
    @Published var message = ""
    @Published var saving = false
    private(set) var frame = ""
    private(set) var requestID = UUID().uuidString
    private var pendingJSON: String?
    private weak var view: ARSCNView?
    private weak var root: SCNNode?
    private var canEdit: () -> Bool = { false }
    private var preview: SCNNode?
    private var baseline: SemanticVolume?
    private var panStart: SIMD3<Float>?

    func attach(_ view: ARSCNView, root: SCNNode, canEdit: @escaping () -> Bool) {
        self.view = view; self.root = root; self.canEdit = canEdit
        let pan = UIPanGestureRecognizer(target: self, action: #selector(pan(_:)))
        pan.maximumNumberOfTouches = 1
        let rotate = UIRotationGestureRecognizer(target: self, action: #selector(rotate(_:)))
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(pinch(_:)))
        for gesture in [pan, rotate, pinch] { gesture.delegate = self; view.addGestureRecognizer(gesture) }
    }
    func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        draft != nil && !saving && pendingJSON == nil && canEdit()
    }
    func floorHit(_ point: CGPoint) -> SIMD3<Float>? {
        guard canEdit(), let view, let root, let camera = view.session.currentFrame?.camera else { return nil }
        let far = view.unprojectPoint(SCNVector3(Float(point.x), Float(point.y), 1))
        let inverse = root.simdTransform.inverse
        let o = inverse * camera.transform.columns.3
        let f = inverse * SIMD4(far.x, far.y, far.z, 1)
        let direction = simd_normalize(f.xyz - o.xyz)
        guard direction.y < -0.15 else { return nil }
        let t = -o.y / direction.y
        guard t > 0, t <= 10 else { return nil }
        return o.xyz + direction * t
    }
    func begin(frame: String, existing: SemanticVolume? = nil) {
        guard canEdit(), let view else { message = "Scan a Portal and wait for tracking."; return }
        if let existing { draft = existing } else {
            guard let hit = floorHit(CGPoint(x: view.bounds.midX, y: view.bounds.midY)) else {
                message = "Aim at the calibrated floor within 10 metres."; return
            }
            draft = SemanticVolume(id: UUID().uuidString, name: "Volume", semantic_type: "region",
                position: [hit.x, 0, hit.z], dimensions: [1, 1, 1], yaw: 0)
        }
        self.frame = frame; requestID = UUID().uuidString; pendingJSON = nil
        message = "Drag to move · pinch footprint · two-finger rotate"
    }
    /// Saved volumes can be hidden from the AR view; a draft preview is always shown.
    var hideSaved = false
    func redraw() {
        preview?.removeFromParentNode()
        root?.childNodes.filter { $0.name?.hasPrefix("semantic-volume:") == true }
            .forEach { $0.isHidden = hideSaved }
        guard let draft, let root else { preview = nil; return }
        root.childNode(withName: "semantic-volume:" + draft.id, recursively: false)?.isHidden = true
        preview = draft.node(preview: true); root.addChildNode(preview!)
    }
    func cancel() {
        draft = nil; baseline = nil; panStart = nil; pendingJSON = nil; saving = false
    }
    func invalidate() {
        guard draft != nil else { return }
        cancel(); message = "Volume draft cancelled because calibration or tracking changed."
    }
    func request(delete: Bool = false) -> String? {
        if let pendingJSON {
            saving = true; message = "Retrying the same operation…"
            return pendingJSON
        }
        guard canEdit(), let draft, draft.valid else { message = "Check tracking, name, type and dimensions."; return nil }
        let action: [String: Any]
        if delete { action = ["kind": "delete_volume", "id": draft.id] }
        else {
            guard let bytes = try? JSONEncoder().encode(draft),
                  let volume = try? JSONSerialization.jsonObject(with: bytes) else { return nil }
            action = ["kind": "put_volume", "volume": volume]
        }
        let body: [String: Any] = ["request_id": requestID, "frame": frame, "action": action]
        guard let bytes = try? JSONSerialization.data(withJSONObject: body),
              let json = String(data: bytes, encoding: .utf8) else { return nil }
        pendingJSON = json; saving = true; message = "Saving to Home…"; return json
    }
    var editable: Bool { !saving && pendingJSON == nil }
    @objc private func pan(_ g: UIPanGestureRecognizer) {
        if g.state == .began { baseline = draft; panStart = floorHit(g.location(in: view)) }
        if g.state == .changed, var value = baseline, let start = panStart,
           let hit = floorHit(g.location(in: view)) {
            value.position[0] += hit.x - start.x; value.position[2] += hit.z - start.z; draft = value
        }
        if g.state == .cancelled { draft = baseline }
    }
    @objc private func rotate(_ g: UIRotationGestureRecognizer) {
        if g.state == .began { baseline = draft }
        if g.state == .changed, var value = baseline {
            let angle = value.yaw - Float(g.rotation)
            value.yaw = atan2(sin(angle), cos(angle)); draft = value
        }
        if g.state == .cancelled { draft = baseline }
    }
    @objc private func pinch(_ g: UIPinchGestureRecognizer) {
        if g.state == .began { baseline = draft }
        if g.state == .changed, var value = baseline {
            for i in [0, 2] { value.dimensions[i] = min(40, max(0.05, value.dimensions[i] * Float(g.scale))) }
            draft = value
        }
        if g.state == .cancelled { draft = baseline }
    }
}

private extension SIMD4 where Scalar == Float {
    var xyz: SIMD3<Float> { SIMD3(x, y, z) }
}

struct VolumeControls: View {
    @ObservedObject var editor: VolumeAuthoring
    @ObservedObject var peer: HomePeer
    let begin: () -> Void
    @State private var confirmDelete = false
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if editor.draft != nil {
                TextField("Name", text: text(\.name))
                TextField("Semantic type", text: text(\.semantic_type))
                ForEach(Array(["Width", "Height", "Depth"].enumerated()), id: \.offset) { i, title in
                    HStack {
                        Text(title)
                        TextField("metres", value: dimension(i), format: .number)
                            .keyboardType(.decimalPad).frame(width: 65)
                        Text("m")
                    }
                }
                .disabled(!editor.editable)
                HStack {
                    Button("Cancel") { editor.cancel() }.disabled(editor.saving)
                    Button("Delete", role: .destructive) { confirmDelete = true }.disabled(!editor.editable)
                    Button(editor.saving ? "Saving…" : "Save / Retry") { save() }.disabled(editor.saving || peer.state != "live")
                }
            } else {
                Button("Create volume", action: begin).disabled(peer.state != "live")
            }
            if !editor.message.isEmpty { Text(editor.message).font(.caption) }
        }
        .padding(12).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .padding().frame(maxWidth: 340)
        .confirmationDialog("Delete this volume from Home?", isPresented: $confirmDelete) {
            Button("Delete volume", role: .destructive) { save(delete: true) }
        }
        .onChange(of: peer.annotationResult) { _, result in
            guard let result, result.id == editor.requestID else { return }
            editor.saving = false
            if result.saved { editor.cancel(); editor.message = "Saved to Home" }
            else { editor.message = result.detail }
        }
    }
    private func text(_ key: WritableKeyPath<SemanticVolume, String>) -> Binding<String> {
        Binding(get: { editor.draft?[keyPath: key] ?? "" },
                set: { if editor.editable { editor.draft?[keyPath: key] = $0 } })
    }
    private func dimension(_ index: Int) -> Binding<Float> {
        Binding(get: { editor.draft?.dimensions[index] ?? 1 },
                set: { if editor.editable { editor.draft?.dimensions[index] = $0 } })
    }
    private func save(delete: Bool = false) {
        guard let json = editor.request(delete: delete) else { return }
        if !peer.changeAnnotation(json) { editor.saving = false; editor.message = "Not queued. Reconnect to Home, then retry." }
    }
}
