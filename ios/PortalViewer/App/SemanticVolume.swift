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
        if surface && semantic_type == Self.circleType {
            node.addChildNode(disc(preview: preview)); node.addChildNode(title())
            return node
        }
        let box = SCNBox(width: CGFloat(dimensions[0]), height: CGFloat(dimensions[1]),
                         length: CGFloat(dimensions[2]), chamferRadius: 0)
        let material = SCNMaterial()
        material.diffuse.contents = preview ? UIColor.white : Theme.uiAccent
        material.lightingModel = .constant
        material.transparency = 0.25
        material.isDoubleSided = true
        material.writesToDepthBuffer = false
        box.materials = [material]
        let body = SCNNode(geometry: box)
        body.position.y = dimensions[1] / 2
        body.name = node.name
        node.addChildNode(body)
        // Solid tube edges. Hairline wireframe over a 25% tint was almost
        // invisible in AR, and from inside a room-sized volume there was
        // nothing to see at all.
        let edgeMaterial = SCNMaterial()
        edgeMaterial.diffuse.contents = preview ? UIColor.white : Theme.uiAccent
        edgeMaterial.lightingModel = .constant
        let (w, h, d) = (dimensions[0], dimensions[1], dimensions[2])
        let radius = CGFloat(max(0.008, min(0.02, min(w, h, d) * 0.04)))
        func tube(length: Float, at position: SIMD3<Float>, axis: SIMD3<Float>) {
            let cylinder = SCNCylinder(radius: radius, height: CGFloat(length))
            cylinder.radialSegmentCount = 8
            cylinder.materials = [edgeMaterial]
            let edge = SCNNode(geometry: cylinder)
            edge.name = node.name
            edge.simdPosition = position
            // SCNCylinder runs along +Y; turn it onto the edge's axis.
            edge.simdOrientation = simd_quatf(from: SIMD3(0, 1, 0), to: axis)
            node.addChildNode(edge)
        }
        for sx: Float in [-1, 1] {
            for sz: Float in [-1, 1] {
                tube(length: h, at: SIMD3(sx * w / 2, h / 2, sz * d / 2), axis: SIMD3(0, 1, 0))
            }
            for y: Float in [0, h] {
                tube(length: d, at: SIMD3(sx * w / 2, y, 0), axis: SIMD3(0, 0, 1))
                tube(length: w, at: SIMD3(0, y, sx * d / 2), axis: SIMD3(1, 0, 0))
            }
        }
        node.addChildNode(title())
        return node
    }
    private func title() -> SCNNode {
        let text = SCNText(string: surface ? name : name + " · " + semantic_type, extrusionDepth: 0)
        text.font = Theme.uiFont(0.18, .semibold)
        text.flatness = 0.01
        text.firstMaterial?.diffuse.contents = UIColor.white
        text.firstMaterial?.lightingModel = .constant
        let label = SCNNode(geometry: text)
        label.name = "semantic-volume:" + id; label.position.y = dimensions[1] + 0.05
        label.constraints = [SCNBillboardConstraint()]
        return label
    }
    /// A surface drawn as a tinted circle with a solid rim, lying flat or standing up.
    private func disc(preview: Bool) -> SCNNode {
        let holder = SCNNode()
        let fill = SCNCylinder(radius: 0.5, height: 0.004)
        fill.radialSegmentCount = 48
        let rim = SCNTorus(ringRadius: 0.5, pipeRadius: 0.012)
        rim.ringSegmentCount = 48
        for (geometry, opacity) in [(fill as SCNGeometry, 0.3), (rim, 1)] {
            let material = SCNMaterial()
            material.diffuse.contents = preview ? UIColor.white : Theme.uiAccent
            material.lightingModel = .constant
            material.transparency = opacity
            material.isDoubleSided = true
            material.writesToDepthBuffer = opacity == 1
            geometry.materials = [material]
            let part = SCNNode(geometry: geometry)
            part.name = "semantic-volume:" + id
            holder.addChildNode(part)
        }
        // The cylinder's axis is +Y: right for the floor, turned onto +Z for a wall.
        if upright { holder.eulerAngles.x = .pi / 2 }
        holder.simdScale = SIMD3(dimensions[0], 1, flat ? dimensions[2] : dimensions[1])
        holder.position.y = dimensions[1] / 2
        return holder
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
    /// The ray through a screen point, in Home's frame.
    private func ray(_ point: CGPoint) -> (origin: SIMD3<Float>, direction: SIMD3<Float>)? {
        guard canEdit(), let view, let root, let camera = view.session.currentFrame?.camera else { return nil }
        let far = view.unprojectPoint(SCNVector3(Float(point.x), Float(point.y), 1))
        let inverse = root.simdTransform.inverse
        let o = inverse * camera.transform.columns.3
        let f = inverse * SIMD4(far.x, far.y, far.z, 1)
        return (o.xyz, simd_normalize(f.xyz - o.xyz))
    }
    func floorHit(_ point: CGPoint) -> SIMD3<Float>? {
        guard let (o, direction) = ray(point), direction.y < -0.15 else { return nil }
        let t = -o.y / direction.y
        guard t > 0, t <= 10 else { return nil }
        return o + direction * t
    }
    /// Where a screen point meets the plane the draft lies in: its wall if it stands up, else its own level.
    private func dragHit(_ point: CGPoint, on volume: SemanticVolume) -> SIMD3<Float>? {
        guard let (o, direction) = ray(point) else { return nil }
        let through = SIMD3(volume.position[0], volume.position[1], volume.position[2])
        let normal = volume.upright ? SIMD3(sin(volume.yaw), 0, cos(volume.yaw)) : SIMD3<Float>(0, 1, 0)
        let facing = simd_dot(direction, normal)
        guard abs(facing) > 0.15 else { return nil }
        let t = simd_dot(through - o, normal) / facing
        guard t > 0, t <= 10 else { return nil }
        return o + direction * t
    }
    func begin(frame: String, existing: SemanticVolume? = nil) {
        guard canEdit(), let view else { message = "Scan a Portal and wait for tracking."; return }
        if let existing { draft = existing } else {
            guard let hit = floorHit(CGPoint(x: view.bounds.midX, y: view.bounds.midY)) else {
                message = "Aim at the calibrated floor within 10 metres."; return
            }
            draft = SemanticVolume(id: UUID().uuidString, name: "Box", semantic_type: "region",
                position: [hit.x, 0, hit.z], dimensions: [1, 1, 1], yaw: 0)
        }
        self.frame = frame; requestID = UUID().uuidString; pendingJSON = nil
        message = draft?.upright == true ? "Drag to move · pinch to resize" : "Drag to move · pinch to resize · twist with two fingers"
    }
    /// A circle on a detected surface, half a metre across unless the surface is smaller than that.
    /// `normal` points out of the surface, in Home's frame.
    /// It is saved as the thinnest volume Home accepts: lying down on a floor or table, standing up on a wall or screen.
    func beginSurface(frame: String, at hit: SIMD3<Float>, normal: SIMD3<Float>, within: Float? = nil) {
        guard canEdit() else { message = "Scan a Portal and wait for tracking."; return }
        let size = max(0.1, min(0.5, within ?? 0.5)), thin = SemanticVolume.flatHeight
        if abs(normal.y) > 0.7 {
            draft = SemanticVolume(id: UUID().uuidString, name: "Circle", semantic_type: SemanticVolume.circleType,
                position: [hit.x, hit.y, hit.z], dimensions: [size, thin, size], yaw: 0)
            message = "Drag to move · pinch to resize · twist with two fingers"
        } else {
            draft = SemanticVolume(id: UUID().uuidString, name: "Circle", semantic_type: SemanticVolume.circleType,
                position: [hit.x, hit.y - size / 2, hit.z], dimensions: [size, size, thin], yaw: atan2(normal.x, normal.z))
            message = "Drag to move · pinch to resize"
        }
        self.frame = frame; requestID = UUID().uuidString; pendingJSON = nil
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
        if g.state == .began { baseline = draft; panStart = draft.flatMap { dragHit(g.location(in: view), on: $0) } }
        if g.state == .changed, var value = baseline, let start = panStart,
           let hit = dragHit(g.location(in: view), on: value) {
            // Along the floor for anything lying down; up and along the wall for anything standing on one.
            for i in value.upright ? [0, 1, 2] : [0, 2] { value.position[i] += hit[i] - start[i] }
            draft = value
        }
        if g.state == .cancelled { draft = baseline }
    }
    @objc private func rotate(_ g: UIRotationGestureRecognizer) {
        if g.state == .began { baseline = draft }
        if g.state == .changed, var value = baseline, !value.upright {
            let angle = value.yaw - Float(g.rotation)
            value.yaw = atan2(sin(angle), cos(angle)); draft = value
        }
        if g.state == .cancelled { draft = baseline }
    }
    @objc private func pinch(_ g: UIPinchGestureRecognizer) {
        if g.state == .began { baseline = draft }
        if g.state == .changed, var value = baseline {
            for i in value.upright ? [0, 1] : [0, 2] { value.dimensions[i] = min(40, max(0.05, value.dimensions[i] * Float(g.scale))) }
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
    @State private var confirmDelete = false
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let draft = editor.draft {
                Text(draft.surface ? "Surface" : "Box").themeLabel(13, .medium)
                TextField("Name", text: text(\.name))
                if draft.surface {
                    TabStrip(tabs: [(SemanticVolume.circleType, "Circle"), ("area", "Rectangle")], selection: text(\.semantic_type))
                } else {
                    TextField("Semantic type", text: text(\.semantic_type))
                }
                ForEach(Array(["Width", "Height", "Depth"].enumerated()), id: \.offset) { i, title in
                    // A surface has no thickness to set.
                    if !(draft.flat && i == 1) && !(draft.upright && i == 2) { HStack {
                        Text(title).themeLabel().foregroundStyle(Theme.dim).frame(width: 60, alignment: .leading)
                        Slider(value: dimension(i), in: 0.05...max(5, draft.dimensions[i]))
                        Text(String(format: "%.2f m", draft.dimensions[i])).themeLabel().foregroundStyle(Theme.dim)
                            .frame(width: 54, alignment: .trailing)
                    } }
                }
                .disabled(!editor.editable)
                HStack {
                    Button("Cancel") { editor.cancel() }.disabled(editor.saving)
                    Button("Delete", role: .destructive) { confirmDelete = true }.disabled(!editor.editable)
                    Button(editor.saving ? "Saving…" : "Save / Retry") { save() }.disabled(editor.saving || peer.state != "live")
                        .buttonStyle(PillButtonStyle(filled: true))
                }
            }
            if !editor.message.isEmpty { Text(editor.message).themeText(10).foregroundStyle(Theme.dim) }
        }
        .textFieldStyle(SoftFieldStyle())
        .buttonStyle(PillButtonStyle())
        .themePanel(padding: 12)
        // Nothing to show until a volume is being edited or there is a result to report.
        .opacity(editor.draft == nil && editor.message.isEmpty ? 0 : 1)
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

/// The always-visible bar for adding to the scene: a point with a speech bubble,
/// a circle on any surface, or a box on the floor.
struct ComponentsBar: View {
    @ObservedObject var controller: ViewerController
    @ObservedObject var editor: VolumeAuthoring
    @ObservedObject var peer: HomePeer
    let connect: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Components").themeLabel(15, .bold)
                Spacer()
                // Always in view, so it is obvious whether volumes are on in AR.
                Button { controller.showVolumesInAR.toggle() } label: {
                    HStack(spacing: 7) {
                        CheckBox(on: controller.showVolumesInAR, size: 16)
                        Text("Show").themeLabel(11, .semibold)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel(controller.showVolumesInAR ? "Hide surfaces and boxes in AR" : "Show surfaces and boxes in AR")
            }
            if peer.running {
                HStack(spacing: 10) {
                    // A point is kept on the phone and synced later, so it does not need Home right now.
                    tool("Point", "bubble.left", "where the phone is", ready: editor.draft == nil) { controller.beginPoint() }
                    tool("Surface", "circle.dashed", "aim at it", ready: canShape) { controller.beginSurface() }
                    tool("Box", "cube", "aim at the floor", ready: canShape) { controller.beginVolume() }
                }
                if peer.state != "live" {
                    Text("Home is out of reach. Points sync when it is back.").themeText(10).foregroundStyle(Theme.dim)
                }
            } else {
                Button("Connect to Home", action: connect)
                    .buttonStyle(PillButtonStyle(filled: true)).frame(maxWidth: .infinity)
            }
        }
        .themePanel(padding: 16)
    }
    private var canShape: Bool { editor.draft == nil && peer.state == "live" }
    private func tool(_ title: String, _ symbol: String, _ how: String, ready: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: symbol).font(.system(size: 20, weight: .semibold)).foregroundStyle(Theme.accent)
                    .frame(height: 24)
                Text(title).themeLabel(11, .semibold).foregroundStyle(Theme.ink)
                Text(how).themeText(8).foregroundStyle(Theme.dim).lineLimit(1).minimumScaleFactor(0.7)
            }
            .frame(maxWidth: .infinity).padding(.vertical, 12)
            .background(Theme.well, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        }
        .buttonStyle(SquishButtonStyle())
        .disabled(!ready).opacity(ready ? 1 : 0.45)
    }
}
