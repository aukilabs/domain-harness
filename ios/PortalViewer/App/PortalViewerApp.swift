import SwiftUI
import ARKit
import SceneKit
import Vision
import CoreMotion
import UniformTypeIdentifiers

@main
struct PortalViewerApp: App {
    var body: some Scene {
        WindowGroup { ViewerScreen() }
    }
}

struct ViewerScreen: View {
    @StateObject private var controller = ViewerController()
    @StateObject private var peer = HomePeer()
    @State private var importing = false
    @State private var connecting = false
    @State private var pairingError: String?
    @AppStorage("panelTab") private var panelTab = "map"
    @AppStorage("panelOpen") private var panelOpen = true
    @State private var promptedForLogin = false
    @Environment(\.scenePhase) private var phase
    var body: some View {
        ZStack(alignment: .bottom) {
            CameraView(controller: controller).ignoresSafeArea()
            if controller.snoozed {
                ZStack {
                    Color.black.opacity(0.88)
                    VStack(spacing: 10) {
                        Image(systemName: "moon.zzz.fill").font(.system(size: 54))
                        Text("Asleep").font(.title.bold())
                        Text("Camera off. Pick up the phone or tap to wake.").font(.subheadline)
                    }.foregroundStyle(.white)
                }
                .ignoresSafeArea()
                .onTapGesture { controller.wake() }
            }
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Text(peer.profile?.sceneLabel ?? "Home Scene").font(.headline).lineLimit(1)
                    Spacer()
                    Picker("Panel", selection: $panelTab) {
                        Text("Map").tag("map")
                        Text("Status").tag("status")
                    }.pickerStyle(.segmented).frame(width: 150)
                    Button { withAnimation { panelOpen.toggle() } } label: {
                        Image(systemName: panelOpen ? "chevron.down" : "chevron.up").frame(width: 30, height: 30)
                    }.accessibilityLabel(panelOpen ? "Fold panel" : "Unfold panel")
                }
                if !panelOpen {
                    Text(controller.status).font(.caption).lineLimit(1)
                } else if panelTab == "map" {
                    MiniMapView(map: controller.miniMap, phone: controller.miniPhone)
                        .frame(height: 260)
                    Text(controller.status).font(.caption).lineLimit(2)
                } else {
                    Text(peer.detail).font(.subheadline.bold())
                    Text(controller.status).font(.subheadline)
                    Toggle("Share LiDAR depth", isOn: $controller.shareDepth)
                    Text(controller.depthStatus).font(.caption)
                    if controller.shareDepth && peer.state == "live" { Text(peer.depthDetail).font(.caption) }
                    HStack {
                        Button(peer.running ? "Disconnect" : "Connect to Home") {
                            if peer.running { peer.disconnect() } else { connecting = true }
                        }
                        Spacer()
                        Button("Realign") { controller.realign() }
                            .disabled(controller.map == nil)
                    }.buttonStyle(.bordered)
                    Text(peer.state == "live" ? "Live P2P · double-tap a surface to add a note" : "Waiting for Home · any visible map is cached")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding().background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20))
            .padding()
        }
        .onAppear {
            peer.onMap = { controller.load($0) }
            controller.onPose = { peer.publishPose($0) }
            controller.onDepth = { peer.publishDepth($0) }
            peer.onDisconnect = { controller.clear() }
            controller.onSleep = { peer.setAsleep($0) }
            // A Portal seen before Home's map has arrived offers sign-in once.
            controller.onPortalSeen = { _ in
                guard !peer.running, !connecting, !promptedForLogin else { return }
                promptedForLogin = true; connecting = true
            }
        }
        .sheet(isPresented: $connecting) {
            NavigationStack {
                Form {
                    Section("Home connection") {
                        ForEach(peer.transferredConnections, id: \.self) { url in
                            Button("Use \(peer.transferredConnectionName(url)) (USB)") {
                                do { try peer.pair(Data(contentsOf: url)); pairingError = nil }
                                catch { pairingError = error.localizedDescription }
                            }
                        }
                        Button("Import Home connection profile") { importing = true }
                        if let profile = peer.profile {
                            Text("Scene: \(profile.sceneLabel)").font(.headline)
                            Text("Peer: \(profile.peer_id)").font(.caption).textSelection(.enabled)
                            Text("Domain: \(profile.domain_id)").font(.caption).textSelection(.enabled)
                        }
                        Text("Select your scene in Home, download its phone connection, then save it to Files on this iPhone.")
                            .font(.caption)
                        if let pairingError { Text(pairingError).foregroundStyle(.red) }
                    }
                    Section("Auki User · development environment") {
                        TextField("Email", text: $peer.email)
                            .textContentType(.username).keyboardType(.emailAddress)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                        SecureField("Password", text: $peer.password).textContentType(.password)
                        Text("Use a User account with access to Home's Domain. Passwords are not saved.")
                            .font(.caption)
                        if peer.profile == nil {
                            Text("Import Home's connection profile above to enable sign-in.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Button("Sign in and connect") {
                            peer.connect()
                            if peer.running { connecting = false }
                        }.disabled(peer.profile == nil || peer.email.isEmpty || peer.password.isEmpty || peer.running)
                    }
                    Text(peer.detail)
                }
                .navigationTitle("Connect to Home")
                .toolbar { Button("Done") { connecting = false } }
            }
            .fileImporter(isPresented: $importing, allowedContentTypes: [.json]) { result in
                do {
                    let url = try result.get()
                    let access = url.startAccessingSecurityScopedResource()
                    defer { if access { url.stopAccessingSecurityScopedResource() } }
                    let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                    guard size <= 32 * 1024 else { throw ViewerError("Connection profile is too large") }
                    try peer.pair(Data(contentsOf: url))
                    pairingError = nil
                } catch { pairingError = error.localizedDescription }
            }
        }
        .sheet(item: $controller.placement, onDismiss: { controller.clearPlacement() }) { placement in
            AnnotationEditor(peer: peer, placement: placement)
        }
        .onChange(of: phase) { _, value in
            if value == .active { controller.start() } else { controller.pause() }
            if value == .background { peer.disconnect(background: true) }
        }
    }
}

/// Top-down plan of Home's scene: the highest voxel in each column, Portals,
/// notes and this phone. Home +X is right and -Z is up, matching a floor Portal.
struct MiniMapData: Equatable {
    struct Cell: Equatable { let x: Float; let z: Float; let rgb: [Float]? }
    var cells = [Cell]()
    var cellSize: Float = 0.1
    var portals = [SIMD2<Float>]()
    var notes = [SIMD2<Float>]()

    init() {}
    init(_ map: PortalMap) {
        let size = map.render?.voxel_size ?? 0.1
        cellSize = size.isFinite && size > 0 ? size : 0.1
        var top = [SIMD2<Int32>: (Float, [Float]?)]()
        for voxel in map.render?.voxels ?? [] {
            guard voxel.center.count == 3, voxel.center.allSatisfy(\.isFinite), voxel.center[1] <= 2.2 else { continue }
            let key = SIMD2(Int32((voxel.center[0] / cellSize).rounded(.down)), Int32((voxel.center[2] / cellSize).rounded(.down)))
            if let old = top[key], old.0 >= voxel.center[1] { continue }
            top[key] = (voxel.center[1], voxel.linear_rgb?.count == 3 ? voxel.linear_rgb : nil)
        }
        cells = top.map { Cell(x: (Float($0.key.x) + 0.5) * cellSize, z: (Float($0.key.y) + 0.5) * cellSize, rgb: $0.value.1) }
        portals = map.scenegraph.anchors.values.map {
            let t = $0.pose_in_map.matrix.columns.3
            return SIMD2(t.x, t.z)
        }
        notes = (map.render?.notes ?? []).compactMap { $0.position.count == 3 ? SIMD2($0.position[0], $0.position[2]) : nil }
    }
}
struct MiniMapPhone: Equatable { let position: SIMD2<Float>; let heading: SIMD2<Float> }

struct MiniMapView: View {
    let map: MiniMapData
    let phone: MiniMapPhone?
    var body: some View {
        Canvas { context, size in
            var points = map.cells.map { SIMD2($0.x, $0.z) } + map.portals + map.notes
            if let phone { points.append(phone.position) }
            guard let first = points.first else {
                context.draw(Text("No map yet. Connect and scan a Portal.").font(.caption).foregroundColor(.secondary),
                             at: CGPoint(x: size.width / 2, y: size.height / 2))
                return
            }
            var low = first, high = first
            for p in points { low = simd_min(low, p); high = simd_max(high, p) }
            // Keep at least a 2 m window so a single Portal is not blown up.
            let center = (low + high) / 2
            let span = simd_max(high - low + SIMD2(repeating: 0.6), SIMD2(repeating: 2))
            let scale = min(Float(size.width) / span.x, Float(size.height) / span.y)
            func place(_ p: SIMD2<Float>) -> CGPoint {
                CGPoint(x: CGFloat((p.x - center.x) * scale) + size.width / 2,
                        y: CGFloat((p.y - center.y) * scale) + size.height / 2)
            }
            let side = CGFloat(map.cellSize * scale) + 0.5
            for cell in map.cells {
                let at = place(SIMD2(cell.x, cell.z))
                let color = cell.rgb.map { Color(.sRGBLinear, red: Double($0[0]), green: Double($0[1]), blue: Double($0[2])) }
                    ?? Color(red: 0.3, green: 0.62, blue: 0.82)
                context.fill(Path(CGRect(x: at.x - side / 2, y: at.y - side / 2, width: side, height: side)), with: .color(color))
            }
            for note in map.notes {
                let at = place(note)
                context.fill(Path(ellipseIn: CGRect(x: at.x - 4, y: at.y - 4, width: 8, height: 8)), with: .color(.white))
                context.stroke(Path(ellipseIn: CGRect(x: at.x - 4, y: at.y - 4, width: 8, height: 8)), with: .color(.black), lineWidth: 1)
            }
            for portal in map.portals {
                let at = place(portal)
                context.fill(Path(CGRect(x: at.x - 5, y: at.y - 5, width: 10, height: 10)), with: .color(.red))
                context.stroke(Path(CGRect(x: at.x - 5, y: at.y - 5, width: 10, height: 10)), with: .color(.white), lineWidth: 1.5)
            }
            if let phone {
                let at = place(phone.position)
                let forward = CGVector(dx: CGFloat(phone.heading.x), dy: CGFloat(phone.heading.y))
                let right = CGVector(dx: -forward.dy, dy: forward.dx)
                var arrow = Path()
                arrow.move(to: CGPoint(x: at.x + forward.dx * 14, y: at.y + forward.dy * 14))
                arrow.addLine(to: CGPoint(x: at.x - forward.dx * 8 + right.dx * 8, y: at.y - forward.dy * 8 + right.dy * 8))
                arrow.addLine(to: CGPoint(x: at.x - forward.dx * 8 - right.dx * 8, y: at.y - forward.dy * 8 - right.dy * 8))
                arrow.closeSubpath()
                context.fill(arrow, with: .color(.cyan))
                context.stroke(arrow, with: .color(.white), lineWidth: 1.5)
            }
        }
        .background(Color.black.opacity(0.55), in: RoundedRectangle(cornerRadius: 12))
        .accessibilityLabel("Top-down map")
    }
}

struct CameraView: UIViewRepresentable {
    let controller: ViewerController
    func makeUIView(context: Context) -> ARSCNView { controller.start(); return controller.view }
    func updateUIView(_ uiView: ARSCNView, context: Context) {}
}

final class ViewerController: NSObject, ObservableObject, ARSessionDelegate {
    @Published var status = "Connect to Home to receive its scene"
    @Published var shareDepth = true {
        didSet { if !shareDepth { onDepth?(nil); depthStatus = "Depth sharing off" } }
    }
    @Published var depthStatus = "Scan a Portal to share depth"
    var onDepth: ((PhoneDepthSample?) -> Void)?
    private let depthQueue = DispatchQueue(label: "home.phone.depth")
    private var depthBusy = false
    private var lastDepthCapture: TimeInterval = 0
    private var depthAlignment: DepthAlignment?
    var onPose: ((PhonePoseSample?) -> Void)?
    var onPortalSeen: ((String) -> Void)?
    @Published private(set) var miniMap = MiniMapData()
    @Published private(set) var miniPhone: MiniMapPhone?
    // A phone left lying flat and still pauses its camera and reports itself asleep.
    static let sleepAfter: TimeInterval = 30
    @Published private(set) var snoozed = false
    var onSleep: ((Bool) -> Void)?
    private var restReference: simd_quatf?
    private var restSince: TimeInterval = 0
    private let motion = CMMotionManager()
    private let poseSession = UUID().uuidString
    private var lastPosePublication: TimeInterval = 0
    @Published var placement: NotePlacement?
    private(set) var map: PortalMap?
    let view = ARSCNView(frame: .zero)
    private let root = SCNNode()
    private let visionQueue = DispatchQueue(label: "home.portal.vision")
    private var busy = false
    private var aligned = false
    private var generation = 0
    private var lastAttempt: TimeInterval = 0
    private var cooldown = CalibrationCooldown()

    override init() {
        super.init()
        view.scene = SCNScene()
        view.scene.rootNode.addChildNode(root)
        view.session.delegate = self
        view.session.delegateQueue = .main
        root.isHidden = true
        let tap = UITapGestureRecognizer(target: self, action: #selector(doubleTap(_:)))
        tap.numberOfTapsRequired = 2
        view.addGestureRecognizer(tap)
        // Long-press means "look for a Portal here, now".
        let press = UILongPressGestureRecognizer(target: self, action: #selector(longPress(_:)))
        press.minimumPressDuration = 0.35
        view.addGestureRecognizer(press)
    }

    func start() {
        guard !snoozed else { return }
        guard ARWorldTrackingConfiguration.isSupported else {
            status = "AR requires a supported physical iPhone"; return
        }
        let config = ARWorldTrackingConfiguration()
        config.worldAlignment = .gravity
        if ARWorldTrackingConfiguration.supportsFrameSemantics(.sceneDepth) {
            config.frameSemantics.insert(.sceneDepth)
        } else { depthStatus = "This phone does not provide LiDAR scene depth" }
        view.session.run(config)
    }

    func pause() {
        view.session.pause()
        invalidate()
    }

    func load(_ map: PortalMap) {
        // Only changes to the calibration reference reset alignment.
        guard self.map != map else { return }
        let referenceChanged = self.map?.scenegraph != map.scenegraph
        self.map = map
        miniMap = MiniMapData(map)
        root.childNodes.forEach { $0.removeFromParentNode() }
        for portal in map.scenegraph.anchors.values.sorted(by: { $0.anchor_id < $1.anchor_id }) {
            let node = SCNNode()
            node.simdTransform = portal.pose_in_map.matrix
            let size = CGFloat(portal.side_length_m)
            // An outline preserves the camera view of the printed marker.
            for (w, h, px, py) in [(size, 0.004, 0.0, size/2), (size, 0.004, 0.0, -size/2),
                                   (0.004, size, size/2, 0.0), (0.004, size, -size/2, 0.0)] {
                let line = SCNNode(geometry: SCNPlane(width: w, height: h))
                line.geometry?.firstMaterial?.diffuse.contents = UIColor.systemRed
                line.geometry?.firstMaterial?.lightingModel = .constant
                line.geometry?.firstMaterial?.isDoubleSided = true
                line.position = SCNVector3(Float(px), Float(py), 0.002)
                node.addChildNode(line)
            }
            let text = SCNText(string: portal.anchor_id, extrusionDepth: 0)
            text.font = .systemFont(ofSize: 1, weight: .semibold)
            text.firstMaterial?.diffuse.contents = UIColor.white
            text.firstMaterial?.lightingModel = .constant
            let label = SCNNode(geometry: text)
            label.scale = SCNVector3(0.018, 0.018, 0.018)
            label.position = SCNVector3(-Float(size/2), Float(size/2) + 0.015, 0)
            let billboard = SCNBillboardConstraint()
            label.constraints = [billboard]
            node.addChildNode(label)
            root.addChildNode(node)
        }
        renderScene(map.render)
        if referenceChanged { realign() }
        else if aligned { status = "Aligned · \(map.scenegraph.anchors.count) Portals · \(map.render?.voxels.count ?? 0) voxels" }
    }

    private func renderScene(_ scene: RenderScene?) {
        guard let scene, scene.frame == map?.scenegraph.map.frame.id else { return }
        // One indexed mesh per chunk avoids a SceneKit node for every voxel.
        let corners: [SIMD3<Float>] = [SIMD3(-1,-1,-1), SIMD3(1,-1,-1), SIMD3(1,1,-1), SIMD3(-1,1,-1),
                                       SIMD3(-1,-1,1), SIMD3(1,-1,1), SIMD3(1,1,1), SIMD3(-1,1,1)]
        let faces: [UInt32] = [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,
                               3,7,6,3,6,2,0,4,7,0,7,3,1,2,6,1,6,5]
        if scene.voxel_size.isFinite && scene.voxel_size > 0 {
            for start in stride(from: 0, to: scene.voxels.count, by: 8192) {
                var vertices = [SCNVector3]()
                var colors = [Float]()
                var indices = [UInt32]()
                for voxel in scene.voxels[start..<min(start + 8192, scene.voxels.count)] {
                    guard voxel.center.count == 3, voxel.center.allSatisfy(\.isFinite) else { continue }
                    let center = SIMD3(voxel.center[0], voxel.center[1], voxel.center[2])
                    let rgb = voxel.linear_rgb?.count == 3 ? voxel.linear_rgb! : [0.08, 0.55, 0.85]
                    let base = UInt32(vertices.count)
                    for corner in corners {
                        let p = center + corner * scene.voxel_size * 0.48
                        vertices.append(SCNVector3(p.x, p.y, p.z))
                        colors.append(contentsOf: rgb + [0.65])
                    }
                    indices.append(contentsOf: faces.map { base + $0 })
                }
                let colorData = colors.withUnsafeBytes { Data($0) }
                let colorSource = SCNGeometrySource(data: colorData, semantic: .color,
                    vectorCount: vertices.count, usesFloatComponents: true, componentsPerVector: 4,
                    bytesPerComponent: 4, dataOffset: 0, dataStride: 16)
                let geometry = SCNGeometry(sources: [SCNGeometrySource(vertices: vertices), colorSource],
                    elements: [SCNGeometryElement(indices: indices, primitiveType: .triangles)])
                let material = SCNMaterial()
                material.lightingModel = .constant
                material.isDoubleSided = true
                geometry.materials = [material]
                root.addChildNode(SCNNode(geometry: geometry))
            }
        }
        for object in scene.objects { addLabel(object.label, at: object.position, color: .systemYellow) }
        for note in scene.notes { addLabel(note.text, at: note.position, color: .white) }
        if let runners = scene.local_mapping?.runners, !runners.isEmpty {
            for (id, runner) in runners { addRobot(runner.robot_pose, name: String(id.suffix(8))) }
        } else { addRobot(scene.local_mapping?.robot_pose, name: "Runner") }
    }

    private func addLabel(_ title: String, at position: [Float], color: UIColor) {
        guard position.count == 3, position.allSatisfy(\.isFinite) else { return }
        let text = SCNText(string: title, extrusionDepth: 0)
        text.font = .systemFont(ofSize: 1, weight: .semibold)
        text.flatness = 0.2
        text.firstMaterial?.diffuse.contents = color
        text.firstMaterial?.lightingModel = .constant
        let label = SCNNode(geometry: text)
        label.scale = SCNVector3(0.05, 0.05, 0.05)
        label.position = SCNVector3(position[0], position[1], position[2])
        label.constraints = [SCNBillboardConstraint()]
        root.addChildNode(label)
    }

    private func addRobot(_ sample: RenderScene.RobotPose?, name: String) {
        guard let pose = sample?.pose, pose.translation.count == 3, pose.rotation_xyzw.count == 4,
              sample?.frame == map?.scenegraph.map.frame.id else { return }
        let robot = SCNNode(geometry: SCNBox(width: 0.35, height: 0.2, length: 0.5, chamferRadius: 0.03))
        robot.geometry?.firstMaterial?.diffuse.contents = UIColor.systemGreen
        robot.geometry?.firstMaterial?.lightingModel = .constant
        robot.simdTransform = pose.matrix
        root.addChildNode(robot)
        var position = pose.translation; position[1] += 0.3
        addLabel(name, at: position, color: .systemGreen)
    }

    func clearPlacement() {
        placement = nil
        root.childNode(withName: "annotation-preview", recursively: false)?.removeFromParentNode()
    }

    @objc private func doubleTap(_ gesture: UITapGestureRecognizer) {
        guard aligned, let map, let frame = view.session.currentFrame,
              case .normal = frame.camera.trackingState, placement == nil else {
            status = "Scan a Portal before placing a note."; return
        }
        let point = gesture.location(in: view)
        let far = view.unprojectPoint(SCNVector3(Float(point.x), Float(point.y), 1))
        let inverse = root.simdTransform.inverse
        let worldOrigin = frame.camera.transform.columns.3
        let localOrigin = inverse * worldOrigin
        let localFar = inverse * SIMD4(far.x, far.y, far.z, 1)
        let origin = SIMD3(localOrigin.x, localOrigin.y, localOrigin.z)
        let direction = simd_normalize(SIMD3(localFar.x, localFar.y, localFar.z) - origin)
        guard let hit = SceneRay.hit(origin: origin, direction: direction, scene: map.render) else {
            status = "No floor or voxel at that point. Tap a surface within 40 metres."; return
        }
        let marker = SCNNode(geometry: SCNSphere(radius: 0.025))
        marker.name = "annotation-preview"
        marker.position = SCNVector3(hit.x, hit.y, hit.z)
        marker.geometry?.firstMaterial?.diffuse.contents = UIColor.systemCyan
        marker.geometry?.firstMaterial?.lightingModel = .constant
        root.addChildNode(marker)
        placement = NotePlacement(id: UUID().uuidString, frame: map.scenegraph.map.frame.id,
            origin: [origin.x, origin.y, origin.z], direction: [direction.x, direction.y, direction.z])
    }

    func clear() {
        clearPlacement()
        map = nil
        miniMap = MiniMapData()
        root.childNodes.forEach { $0.removeFromParentNode() }
        invalidate()
        status = "Connect to Home to receive its scene"
    }

    private func invalidate() {
        onPose?(nil)
        onDepth?(nil)
        depthAlignment = nil
        depthStatus = shareDepth ? "Scan a Portal to share depth" : "Depth sharing off"
        generation += 1; aligned = false; cooldown.reset(); root.isHidden = true
        miniPhone = nil
    }

    func realign() {
        invalidate()
        status = "\(map?.scenegraph.anchors.count ?? 0) Portals loaded. Scan a known Portal."
    }

    private var toast: UILabel?
    private func showToast(_ text: String, good: Bool) {
        toast?.removeFromSuperview()
        let label = UILabel()
        label.text = text; label.numberOfLines = 0; label.textAlignment = .center
        label.font = .systemFont(ofSize: 17, weight: .semibold); label.textColor = .white
        label.backgroundColor = (good ? UIColor.systemGreen : UIColor.systemOrange).withAlphaComponent(0.92)
        label.layer.cornerRadius = 14; label.layer.masksToBounds = true
        let width = view.bounds.width - 40
        let height = label.sizeThatFits(CGSize(width: width - 24, height: 400)).height + 24
        label.frame = CGRect(x: 20, y: view.safeAreaInsets.top + 12, width: width, height: height)
        view.addSubview(label); toast = label
        UIView.animate(withDuration: 0.4, delay: 2.6, options: []) { label.alpha = 0 } completion: { _ in
            label.removeFromSuperview()
        }
    }
    private func showOutline(_ points: [CGPoint], good: Bool) {
        guard points.count == 4 else { return }
        let path = UIBezierPath(); path.move(to: points[0])
        points.dropFirst().forEach { path.addLine(to: $0) }; path.close()
        let layer = CAShapeLayer()
        layer.path = path.cgPath; layer.fillColor = UIColor.clear.cgColor; layer.lineWidth = 5
        layer.strokeColor = (good ? UIColor.systemGreen : UIColor.systemOrange).cgColor
        view.layer.addSublayer(layer)
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 1; fade.toValue = 0; fade.beginTime = CACurrentMediaTime() + 1.2; fade.duration = 0.6
        fade.fillMode = .forwards; fade.isRemovedOnCompletion = false
        layer.add(fade, forKey: "fade")
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { layer.removeFromSuperlayer() }
    }

    /// Explicit user scan: always gives visible feedback, and forces the next
    /// alignment attempt instead of waiting for the cooldown.
    @objc private func longPress(_ gesture: UILongPressGestureRecognizer) {
        guard gesture.state == .began else { return }
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        guard let frame = view.session.currentFrame else {
            showToast("Camera is not ready yet", good: false); return
        }
        showToast("Looking for a Portal…", good: true)
        let size = view.bounds.size
        let orientation = view.window?.windowScene?.interfaceOrientation ?? .portrait
        let display = frame.displayTransform(for: orientation, viewportSize: size)
        let image = frame.capturedImage
        let map = self.map
        visionQueue.async {
            let request = VNDetectBarcodesRequest()
            request.symbologies = [.qr]
            try? VNImageRequestHandler(cvPixelBuffer: image, orientation: .up).perform([request])
            let codes = (request.results ?? []).map { code -> (String?, [CGPoint]) in
                let corners = [code.topLeft, code.topRight, code.bottomRight, code.bottomLeft].map { p -> CGPoint in
                    // Vision: origin bottom-left. ARKit image space: origin top-left.
                    let shown = CGPoint(x: p.x, y: 1 - p.y).applying(display)
                    return CGPoint(x: shown.x * size.width, y: shown.y * size.height)
                }
                return (code.payloadStringValue, corners)
            }
            DispatchQueue.main.async {
                guard let (payload, corners) = codes.first(where: { $0.0.flatMap(PortalMap.portalCode) != nil }) ?? codes.first else {
                    UINotificationFeedbackGenerator().notificationOccurred(.warning)
                    self.showToast("No QR code in view. Get the whole code in frame and hold still.", good: false)
                    return
                }
                guard let payload, let code = PortalMap.portalCode(payload) else {
                    self.showOutline(corners, good: false)
                    UINotificationFeedbackGenerator().notificationOccurred(.warning)
                    self.showToast("That QR code is not an Auki Portal.", good: false)
                    return
                }
                guard let map else {
                    self.showOutline(corners, good: true)
                    UINotificationFeedbackGenerator().notificationOccurred(.success)
                    self.showToast("Portal \(code) found. Connecting to Home…", good: true)
                    self.onPortalSeen?(code)
                    return
                }
                guard map.matching(payload) != nil else {
                    self.showOutline(corners, good: false)
                    UINotificationFeedbackGenerator().notificationOccurred(.warning)
                    self.showToast("Portal \(code) is not part of this scene.", good: false)
                    return
                }
                self.showOutline(corners, good: true)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                self.showToast("Portal \(code) found. Aligning…", good: true)
                // Scan again on the very next frame, ignoring the cooldown.
                self.cooldown.reset(); self.lastAttempt = 0
            }
        }
    }

    /// Flat (screen up or down) and rotated less than about two degrees.
    private func checkRest(_ frame: ARFrame) {
        let transform = frame.camera.transform
        let flat = abs(transform.columns.2.y) > 0.9
        let orientation = simd_quatf(transform)
        guard flat, let reference = restReference,
              abs(simd_dot(reference.vector, orientation.vector)) > 0.99985 else {
            restReference = flat ? orientation : nil
            restSince = frame.timestamp
            return
        }
        if frame.timestamp - restSince >= Self.sleepAfter { snooze() }
    }

    private func snooze() {
        guard !snoozed else { return }
        snoozed = true
        // Report sleep before withdrawing the pose so Home keeps the last one.
        onSleep?(true)
        view.session.pause()
        invalidate()
        restReference = nil
        status = "Asleep · camera off. Pick up the phone or tap to wake."
        guard motion.isDeviceMotionAvailable else { return }
        motion.deviceMotionUpdateInterval = 0.2
        motion.startDeviceMotionUpdates(to: .main) { [weak self] sample, _ in
            guard let sample else { return }
            let a = sample.userAcceleration
            let moved = (a.x * a.x + a.y * a.y + a.z * a.z).squareRoot() > 0.12
            if abs(sample.gravity.z) < 0.8 || moved { self?.wake() }
        }
    }

    func wake() {
        guard snoozed else { return }
        motion.stopDeviceMotionUpdates()
        snoozed = false
        onSleep?(false)
        status = map == nil ? "Connect to Home to receive its scene" : "Awake. Scan a known Portal."
        start()
    }

    /// Before Home's map arrives: read Portal QR codes anyway so the user gets
    /// feedback and sign-in can start. No alignment is attempted here.
    private func scanBeforeMap(_ frame: ARFrame) {
        guard !busy, frame.timestamp - lastAttempt > 0.5 else { return }
        busy = true; lastAttempt = frame.timestamp
        let capturedGeneration = generation
        let image = frame.capturedImage
        visionQueue.async {
            let request = VNDetectBarcodesRequest()
            request.symbologies = [.qr]
            try? VNImageRequestHandler(cvPixelBuffer: image, orientation: .up).perform([request])
            let code = (request.results ?? []).compactMap { $0.payloadStringValue }
                .compactMap { PortalMap.portalCode($0) }.first
            DispatchQueue.main.async {
                self.busy = false
                guard self.generation == capturedGeneration, self.map == nil, let code else { return }
                self.status = "Portal \(code) in view. It aligns as soon as Home's map arrives."
                self.onPortalSeen?(code)
            }
        }
    }

    func session(_ session: ARSession, didUpdate frame: ARFrame) {
        if snoozed { return }
        checkRest(frame)
        if snoozed { return }
        if map == nil { scanBeforeMap(frame); return }
        guard case .normal = frame.camera.trackingState else {
            if map != nil {
                let message: String
                switch frame.camera.trackingState {
                case .limited(.initializing): message = "Camera tracking is starting. Move the phone slowly."
                case .limited(.insufficientFeatures): message = "Camera needs more detail. Include the Portal's surroundings."
                case .limited(.excessiveMotion): message = "Move the phone more slowly to scan."
                case .limited(.relocalizing): message = "Recovering camera tracking. Look around slowly."
                default: message = "Camera tracking unavailable. Tap Realign to try again."
                }
                if status != message { status = message }
            }
            return
        }
        if aligned {
            root.isHidden = false
            captureDepth(frame)
            if let map, frame.timestamp - lastPosePublication >= 0.2 {
                lastPosePublication = frame.timestamp
                let cameraInHome = root.simdTransform.inverse * frame.camera.transform
                let forward = SIMD2(-cameraInHome.columns.2.x, -cameraInHome.columns.2.z)
                // Looking straight down has no horizontal heading; use the screen's top edge.
                // In portrait the top edge is the camera's -X axis.
                let up = SIMD2(-cameraInHome.columns.0.x, -cameraInHome.columns.0.z)
                let heading = simd_length(forward) > 0.3 ? simd_normalize(forward) : simd_normalize(up)
                miniPhone = MiniMapPhone(position: SIMD2(cameraInHome.columns.3.x, cameraInHome.columns.3.z),
                                         heading: heading.x.isFinite ? heading : SIMD2(0, -1))
                onPose?(PhonePoseSample(session: poseSession, timestamp: frame.timestamp,
                    cameraInAR: frame.camera.transform, homeInAR: root.simdTransform,
                    homeFrame: map.scenegraph.map.frame.id))
            }
        }
        guard placement == nil, let map, !busy, cooldown.canScan(at: frame.timestamp),
              frame.timestamp - lastAttempt > 0.25 else { return }
        busy = true; lastAttempt = frame.timestamp
        let capturedGeneration = generation
        // Keep image, intrinsics and camera transform from the SAME ARFrame.
        visionQueue.async {
            let request = VNDetectBarcodesRequest()
            request.symbologies = [.qr]
            var feedback = "No QR detected. Move closer and keep the whole Portal visible."
            do {
                try VNImageRequestHandler(cvPixelBuffer: frame.capturedImage, orientation: .up).perform([request])
            } catch {
                feedback = "Camera scan failed. Tap Realign to try again."
            }
            let width = Float(CVPixelBufferGetWidth(frame.capturedImage))
            let height = Float(CVPixelBufferGetHeight(frame.capturedImage))
            var found: (String, simd_float4x4, simd_float4x4)?
            for code in request.results ?? [] {
                guard let payload = code.payloadStringValue else {
                    feedback = "QR visible but unreadable. Move closer and avoid glare."
                    continue
                }
                guard let portal = map.matching(payload) else {
                    if let code = PortalMap.portalCode(payload) {
                        feedback = "Portal \(code) is missing from Home's current snapshot."
                    } else {
                        feedback = "QR detected, but its payload format is not recognized as a Portal."
                    }
                    continue
                }
                feedback = "Recognized \(portal.anchor_id). Face it more directly and hold still."
                let corners = [code.topLeft, code.topRight, code.bottomRight, code.bottomLeft]
                    .map { SIMD2(Float($0.x) * width, (1 - Float($0.y)) * height) }
                guard let pose = PortalPose.estimate(corners: corners, side: portal.side_length_m,
                                                     intrinsics: frame.camera.intrinsics) else { continue }
                let portalInAR = frame.camera.transform * pose
                found = (portal.anchor_id, portalInAR * portal.pose_in_map.matrix.inverse, portalInAR)
                break
            }
            let result = found
            let scanFeedback = feedback
            DispatchQueue.main.async {
                self.busy = false
                guard self.generation == capturedGeneration, self.placement == nil else { return }
                guard case .normal = self.view.session.currentFrame?.camera.trackingState else { return }
                guard let (id, transform, portalInAR) = result else {
                    if !self.aligned { self.status = scanFeedback }
                    return
                }
                self.root.simdTransform = transform
                self.root.isHidden = false
                self.aligned = true
                if let portal = map.scenegraph.anchors[id] {
                    let sessionFrame = "phone-ar:\(self.poseSession)"
                    self.depthAlignment = DepthAlignment(timestamp_ns: UInt64(frame.timestamp * 1_000_000_000), portal_id: id,
                        portal_to_session: .transform(portalInAR, from: portal.pose_in_map.from_frame_id, to: sessionFrame),
                        session_to_home: .transform(transform.inverse, from: sessionFrame, to: map.scenegraph.map.frame.id))
                }
                // Start the one-second pause when calibration is applied, not
                // when Vision began processing the captured frame.
                self.cooldown.accept(at: self.view.session.currentFrame?.timestamp ?? frame.timestamp)
                self.status = "Aligned with \(id) · \(self.map?.render?.voxels.count ?? 0) voxels"

            }
        }
    }

    private func captureDepth(_ frame: ARFrame) {
        guard shareDepth, !depthBusy, let alignment = depthAlignment,
              frame.timestamp - lastDepthCapture >= 0.5 else { return }
        guard frame.sceneDepth != nil else {
            depthStatus = "Waiting for LiDAR depth"; return
        }
        lastDepthCapture = frame.timestamp
        depthBusy = true
        let capturedGeneration = generation
        let session = poseSession
        depthQueue.async {
            let sample = PhoneDepthCapture.read(frame, alignment: alignment, session: session)
            DispatchQueue.main.async {
                self.depthBusy = false
                guard self.generation == capturedGeneration, self.aligned, self.shareDepth else { return }
                guard let sample else { self.depthStatus = "No reliable depth here. Aim at nearby surfaces."; return }
                self.depthStatus = "LiDAR active · \(sample.points.count) points per capture"
                self.onDepth?(sample)
            }
        }
    }

    func session(_ session: ARSession, cameraDidChangeTrackingState camera: ARCamera) {
        if snoozed { return }
        if case .normal = camera.trackingState { return }
        invalidate()
        status = "Tracking limited. Move slowly, then scan a known Portal again."
    }
    func sessionWasInterrupted(_ session: ARSession) {
        if snoozed { return }
        invalidate(); status = "AR interrupted. Rescan a Portal when the camera returns."
    }
    func sessionInterruptionEnded(_ session: ARSession) { if snoozed { return }; realign(); start() }
    func session(_ session: ARSession, didFailWithError error: Error) {
        invalidate(); status = error.localizedDescription
    }
}


struct AnnotationEditor: View {
    @ObservedObject var peer: HomePeer
    let placement: NotePlacement
    @State private var text = ""
    @State private var saving = false
    @State private var message = ""
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        NavigationStack {
            Form {
                TextField("Annotation", text: $text, axis: .vertical)
                Text("Saved to Home and shared with connected viewers.").font(.caption)
                if !message.isEmpty { Text(message) }
                Button(saving ? "Saving…" : "Save to Home") {
                    saving = peer.saveAnnotation(placement, text: text.trimmingCharacters(in: .whitespacesAndNewlines))
                    message = saving ? "" : "Connect to Home before saving."
                }.disabled(saving || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || text.count > 120)
            }
            .navigationTitle("Add annotation")
            .toolbar { Button("Cancel") { dismiss() }.disabled(saving) }
            .interactiveDismissDisabled(saving)
            .onChange(of: peer.annotationResult) { _, result in
                guard let result, result.id == placement.id else { return }
                saving = false
                if result.saved { dismiss() } else { message = result.detail }
            }
            .onChange(of: peer.state) { _, state in
                if state != "live" { saving = false; message = "Home disconnected. Reconnect before saving." }
            }
        }
    }
}


enum PhoneDepthCapture {
    static func read(_ frame: ARFrame, alignment: DepthAlignment, session: String) -> PhoneDepthSample? {
        guard let depth = frame.sceneDepth, let confidence = depth.confidenceMap else { return nil }
        let buffer = depth.depthMap
        guard CVPixelBufferGetPixelFormatType(buffer) == kCVPixelFormatType_DepthFloat32,
              CVPixelBufferGetPixelFormatType(confidence) == kCVPixelFormatType_OneComponent8 else { return nil }
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        CVPixelBufferLockBaseAddress(confidence, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(confidence, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer), let confidenceBase = CVPixelBufferGetBaseAddress(confidence) else { return nil }
        let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
        guard CVPixelBufferGetWidth(confidence) == width, CVPixelBufferGetHeight(confidence) == height else { return nil }
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer), confidenceRow = CVPixelBufferGetBytesPerRow(confidence)
        let step = max(1, Int(ceil(sqrt(Double(width * height) / 1500))))
        let imageWidth = CVPixelBufferGetWidth(frame.capturedImage), imageHeight = CVPixelBufferGetHeight(frame.capturedImage)
        var points = [[Float]]()
        // Color comes from the same frame's camera image (bi-planar YCbCr).
        let image = frame.capturedImage
        let format = CVPixelBufferGetPixelFormatType(image)
        let fullRange = format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        let colorable = (fullRange || format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)
            && CVPixelBufferGetPlaneCount(image) == 2
        if colorable { CVPixelBufferLockBaseAddress(image, .readOnly) }
        defer { if colorable { CVPixelBufferUnlockBaseAddress(image, .readOnly) } }
        let luma = colorable ? CVPixelBufferGetBaseAddressOfPlane(image, 0) : nil
        let chroma = colorable ? CVPixelBufferGetBaseAddressOfPlane(image, 1) : nil
        let lumaRow = colorable ? CVPixelBufferGetBytesPerRowOfPlane(image, 0) : 0
        let chromaRow = colorable ? CVPixelBufferGetBytesPerRowOfPlane(image, 1) : 0
        var colors = [[UInt8]]()
        func color(x: Int, y: Int) -> [UInt8]? {
            guard let luma, let chroma else { return nil }
            let px = min(imageWidth - 1, x * imageWidth / width), py = min(imageHeight - 1, y * imageHeight / height)
            var yValue = Float(luma.load(fromByteOffset: py * lumaRow + px, as: UInt8.self))
            let offset = (py / 2) * chromaRow + (px / 2) * 2
            var cb = Float(chroma.load(fromByteOffset: offset, as: UInt8.self)) - 128
            var cr = Float(chroma.load(fromByteOffset: offset + 1, as: UInt8.self)) - 128
            if !fullRange { yValue = (yValue - 16) * 255 / 219; cb *= 255 / 224; cr *= 255 / 224 }
            // BT.601 YCbCr to sRGB, as ARKit's camera image is specified.
            let r = yValue + 1.402 * cr, g = yValue - 0.344136 * cb - 0.714136 * cr, b = yValue + 1.772 * cb
            return [r, g, b].map { UInt8(max(0, min(255, $0.rounded()))) }
        }
        for y in stride(from: 0, to: height, by: step) {
            let depths = base.advanced(by: y * rowBytes).assumingMemoryBound(to: Float32.self)
            let confidences = confidenceBase.advanced(by: y * confidenceRow).assumingMemoryBound(to: UInt8.self)
            for x in stride(from: 0, to: width, by: step) {
                guard confidences[x] >= UInt8(ARConfidenceLevel.high.rawValue),
                      let point = DepthProjection.point(x: x, y: y, depth: depths[x], width: width, height: height,
                          imageWidth: imageWidth, imageHeight: imageHeight, intrinsics: frame.camera.intrinsics) else { continue }
                points.append(point)
                if let rgb = color(x: x, y: y) { colors.append(rgb) }
                if points.count == 2048 { break }
            }
            if points.count == 2048 { break }
        }
        guard !points.isEmpty else { return nil }
        return PhoneDepthSample(session: session, timestamp_ns: UInt64(frame.timestamp * 1_000_000_000),
            camera_to_session: .transform(frame.camera.transform, from: "phone-camera:\(session)", to: "phone-ar:\(session)"),
            alignment: alignment, points: points,
            colors: colors.count == points.count ? colors : nil)
    }
}
