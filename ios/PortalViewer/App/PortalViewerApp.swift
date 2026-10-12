import SwiftUI
import ARKit
import SceneKit
import Vision
import CoreMotion
import UniformTypeIdentifiers
import PhotosUI

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
    @State private var povSelection: String?
    @AppStorage("phoneName") private var phoneName = ""
    @State private var driveSelection: String?
    @AppStorage("panelTab") private var panelTab = "map"
    @AppStorage("panelOpen") private var panelOpen = true
    /// Off by default: most people only need the Components bar.
    @AppStorage("showDetails") private var showDetails = false
    @State private var promptedForLogin = false
    @Environment(\.scenePhase) private var phase
    /// Whether voxels are reaching Home right now: ink sending, orange failing, grey idle.
    private var sending: Bool { peer.state == "live" && peer.depthDetail.hasPrefix("Depth sent") }
    private var sendFailed: Bool {
        peer.state == "live" && (peer.depthDetail.hasPrefix("Depth not sent") || peer.depthDetail.hasPrefix("Color paused"))
    }
    private var sendColor: Color { sending ? Theme.ink : sendFailed ? Theme.accent : Theme.dim }
    private var sendTag: String { sending ? "Sending" : sendFailed ? "Not sending" : "Idle" }
    /// Robots that have a pose in this scene, for the mini map.
    private var mapRobots: [MiniMapRobot] {
        controller.devices.compactMap { device in
            device.chassis.map { MiniMapRobot(name: device.name, transform: $0, zUp: device.chassisZUp) }
        }
    }
    /// Asleep, the camera is off, so the panel grows from the bottom and the map takes the screen.
    private var tab: String { controller.snoozed ? "map" : panelTab }
    private var open: Bool { controller.snoozed || panelOpen }
    private var mapHeight: CGFloat { controller.snoozed ? max(260, UIScreen.main.bounds.height - 350) : 260 }
    private var sendSummary: String {
        if controller.snoozed { return "Not sending · phone asleep" }
        if peer.state != "live" { return "Not sending · not connected to Home" }
        if !controller.shareDepth { return "Not sending · sharing is off" }
        if sending || sendFailed { return peer.depthDetail }
        return "Not sending · scan a Portal to align"
    }
    var body: some View {
        ZStack(alignment: .bottom) {
            CameraView(controller: controller).ignoresSafeArea()
            VStack {
                HStack(alignment: .top) {
                    VolumeControls(editor: controller.volumes, peer: peer)
                    Spacer(minLength: 0)
                    Button { withAnimation(.spring(response: 0.35, dampingFraction: 0.75)) { showDetails.toggle() } } label: {
                        Image(uiImage: UIImage(named: "auki.png") ?? UIImage()).renderingMode(.original).resizable()
                            .frame(width: 44, height: 44).clipShape(Circle())
                            .overlay(Circle().stroke(showDetails ? Theme.accent : Theme.paper, lineWidth: 3))
                            .shadow(color: Theme.glow, radius: 6, y: 3)
                            // North, circling the logo once it has been found.
                            .overlay {
                                if let turn = controller.northTurn {
                                    Path { path in
                                        path.move(to: CGPoint(x: 3, y: 0))
                                        path.addLine(to: CGPoint(x: 6, y: 10))
                                        path.addLine(to: CGPoint(x: 0, y: 10))
                                        path.closeSubpath()
                                    }
                                        .fill(Color.red).frame(width: 6, height: 10)
                                        .offset(y: -31).rotationEffect(.degrees(turn))
                                        .animation(.linear(duration: 0.12), value: turn)
                                        .accessibilityHidden(true)
                                }
                            }
                    }
                    .padding()
                    .accessibilityLabel(showDetails ? "Hide scene details" : "Show scene details")
                    .opacity(controller.snoozed ? 0 : 1).disabled(controller.snoozed)
                }
                Spacer()
            }
            if controller.snoozed {
                ZStack {
                    Theme.paper.opacity(0.95).ignoresSafeArea()
                    HStack(spacing: 14) {
                        HStack(alignment: .top, spacing: 2) {
                            Image(systemName: "iphone").font(.system(size: 34, weight: .light))
                            Image(systemName: "moon.zzz").font(.system(size: 16, weight: .regular)).foregroundStyle(Theme.accent)
                        }
                        VStack(alignment: .leading, spacing: 5) {
                            Text("Asleep").themeLabel(17, .medium)
                            Text("Camera off. Pick up the phone or tap here to wake.").themeText(11).foregroundStyle(Theme.dim)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(16).overlay(Brackets(arm: 12).stroke(Theme.ink, style: StrokeStyle(lineWidth: 2.5, lineCap: .round)))
                    .foregroundStyle(Theme.ink)
                    // Top of the screen; the map panel fills the rest.
                    .frame(maxHeight: .infinity, alignment: .top).padding(.top, 8).padding(.horizontal, 16)
                }
                .onTapGesture { controller.wake() }
            }
            VStack(spacing: 12) {
                // Naming a point takes the bar's place, so the field sits just above the keyboard.
                if let placement = controller.placement {
                    AnnotationEditor(controller: controller, peer: peer, placement: placement, close: { controller.clearPlacement() })
                        .id(placement.id)
                } else if !controller.snoozed {
                    ComponentsBar(controller: controller, editor: controller.volumes, peer: peer, connect: { connecting = true })
                }
                // The scene panel is more than most people need; the Auki button brings it up.
                if (showDetails && controller.placement == nil) || controller.snoozed {
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(spacing: 9) {
                            CheckBox(on: sending || sendFailed, color: sendColor, size: 9)
                                .accessibilityLabel(sendSummary)
                            Text(peer.profile?.sceneLabel ?? "Home Scene").themeLabel(15, .medium).lineLimit(1)
                            Spacer()
                            Text(sendTag).themeLabel(10, .semibold).foregroundStyle(sendColor).accessibilityHidden(true)
                            Button { withAnimation { panelOpen.toggle() } } label: {
                                Image(systemName: panelOpen ? "chevron.down" : "chevron.up")
                                    .font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink)
                                    .frame(width: 30, height: 30).background(Theme.well, in: Circle())
                            }.accessibilityLabel(panelOpen ? "Fold panel" : "Unfold panel")
                                .opacity(controller.snoozed ? 0 : 1).disabled(controller.snoozed)
                        }
                        if open && !controller.snoozed {
                            TabStrip(tabs: [("map", "Map"), ("pov", "POV"), ("drive", "Drive"), ("status", "Status")],
                                     selection: $panelTab)
                        }
                        if !open {
                            Text(sendSummary).themeText(11).foregroundStyle(Theme.dim).lineLimit(1)
                        } else if tab == "map" {
                            MiniMapView(map: controller.miniMap, phone: controller.miniPhone, robots: mapRobots,
                                        tracked: controller.trackedDevices, showTrails: $controller.showTrails,
                                        clearTrails: controller.clearTrails)
                                .frame(height: mapHeight)
                            Text(sendSummary).themeLabel(10, .semibold).foregroundStyle(sendColor).lineLimit(2)
                            Text(controller.status).themeText(11).foregroundStyle(Theme.dim).lineLimit(2)
                        } else if tab == "drive" {
                            DrivePanel(controller: controller, peer: peer, selection: $driveSelection).frame(height: 260)
                        } else if tab == "pov" {
                            PovPanel(controller: controller, selection: $povSelection).frame(height: 260)
                            Text(sendSummary).themeLabel(10, .semibold).foregroundStyle(sendColor).lineLimit(1)
                        } else {
                            Text(peer.detail).themeText(12, .semibold)
                            Text(controller.status).themeText(11).foregroundStyle(Theme.dim)
                            if !controller.geoStatus.isEmpty {
                                Text(controller.geoStatus).themeText(10).foregroundStyle(Theme.dim)
                            }
                            HStack(spacing: 10) {
                                Text("Name").themeLabel().foregroundStyle(Theme.dim)
                                TextField("How Home labels this phone", text: $phoneName)
                                    .textFieldStyle(SoftFieldStyle()).autocorrectionDisabled()
                                    .textInputAutocapitalization(.never).submitLabel(.done)
                                    .onSubmit { peer.setName(phoneName) }
                            }
                            Dots()
                            Text("Layers").themeLabel().foregroundStyle(Theme.dim)
                            Toggle("Semantic volumes in AR", isOn: $controller.showVolumesInAR)
                            Toggle("Share LiDAR depth", isOn: $controller.shareDepth)
                            Text(controller.depthStatus).themeText(10).foregroundStyle(Theme.dim)
                            if controller.shareDepth && peer.state == "live" {
                                Text(peer.depthDetail).themeText(10).foregroundStyle(Theme.dim)
                            }
                            Dots()
                            HStack {
                                Button(peer.running ? "Disconnect" : "Connect to Home") {
                                    if peer.running { peer.disconnect() } else { connecting = true }
                                }.buttonStyle(PillButtonStyle(filled: !peer.running))
                                Spacer()
                                Button("Realign") { controller.realign() }
                                    .disabled(controller.map == nil)
                            }.buttonStyle(PillButtonStyle())
                            Text(peer.state == "live" ? "Live P2P · double-tap a surface to add a note" : "Waiting for Home · any visible map is cached")
                                .themeText(10).foregroundStyle(Theme.dim)
                        }
                    }
                    .toggleStyle(CheckToggleStyle())
                    .themePanel()
                }
            }
            .padding()
        }
        .animation(.easeInOut(duration: 0.3), value: controller.snoozed)
        .onAppear {
            peer.onMap = { controller.load($0) }
            controller.onPose = { peer.publishPose($0) }
            controller.onDepth = { peer.publishDepth($0) }
            peer.onDisconnect = { controller.clear() }
            controller.onSleep = { peer.setAsleep($0) }
            controller.onNewPortal = { peer.reportPortal($0) }
            peer.onPortalResult = { controller.showPortalResult($0) }
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
                        if let pairingError { Text(pairingError).foregroundStyle(Theme.accent) }
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
                .themeForm()
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
        .onChange(of: phase) { _, value in
            if value == .active { controller.start() } else { controller.pause() }
            // Leaving the foreground always lets go of the joystick.
            if value != .active { peer.drive(target: nil) }
            if value == .background { peer.disconnect(background: true) }
        }
    }
}

/// Hold-to-drive joystick for one robot. Motion only while a finger is on the
/// stick; lifting it, leaving this tab, or leaving the app releases it. The
/// robot itself decides whether this phone may drive and how fast.
struct DrivePanel: View {
    @ObservedObject var controller: ViewerController
    @ObservedObject var peer: HomePeer
    @Binding var selection: String?
    @State private var armed = false
    @State private var stick = CGSize.zero
    private let radius: CGFloat = 78

    /// Devices Home lists by peer ID that are not phones with a pose.
    private var robots: [(id: String, name: String)] {
        controller.devices.compactMap { device in
            for prefix in ["robot:", "peer:"] where device.id.hasPrefix(prefix) {
                return (String(device.id.dropFirst(prefix.count)), device.name)
            }
            return nil
        }
    }

    var body: some View {
        let robots = robots
        let chosen = robots.first { $0.id == selection } ?? robots.first
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 8) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(robots, id: \.id) { robot in
                            Button { release(); selection = robot.id } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: "car.side").frame(width: 18)
                                    Text(robot.name).themeLabel(10).lineLimit(2)
                                    Spacer(minLength: 0)
                                }
                                .themeRow(selected: robot.id == chosen?.id)
                            }.buttonStyle(.plain)
                        }
                        if robots.isEmpty {
                            Text("No robot in this scene yet.").themeText(10).foregroundStyle(Theme.dim)
                        }
                    }
                }
                Toggle("Arm", isOn: $armed)
                    .disabled(chosen == nil || peer.state != "live")
            }.frame(width: 118)
            VStack(spacing: 6) {
                ZStack {
                    Circle().fill(Theme.well)
                    // Crosshair through the rest position.
                    Rectangle().fill(Theme.rule).frame(width: 0.5)
                    Rectangle().fill(Theme.rule).frame(height: 0.5)
                    Circle().stroke(Theme.rule, style: StrokeStyle(lineWidth: 0.5, dash: [3, 3])).frame(width: radius, height: radius)
                    Circle().stroke(armed ? Theme.accent : Theme.dim, lineWidth: armed ? 1.5 : 1)
                    Circle().fill(armed ? Theme.accent : Theme.dim).frame(width: 54, height: 54).offset(stick)
                    Circle().stroke(Theme.paper, lineWidth: 1).frame(width: 54, height: 54).offset(stick)
                }
                .clipShape(Circle())
                .frame(width: radius * 2, height: radius * 2)
                .padding(4).overlay(Brackets(arm: 12).stroke(armed ? Theme.ink : Theme.rule, style: StrokeStyle(lineWidth: 2.5, lineCap: .round)))
                .contentShape(Circle())
                .gesture(DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        guard armed, let chosen, peer.state == "live" else { return }
                        var dx = value.location.x - radius, dy = value.location.y - radius
                        let length = max(1, hypot(dx, dy))
                        if length > radius { dx *= radius / length; dy *= radius / length }
                        stick = CGSize(width: dx, height: dy)
                        // Up is forward. Left is a counter-clockwise turn, which is positive.
                        peer.drive(target: chosen.id, forward: Float(-dy / radius), turn: Float(-dx / radius))
                    }
                    .onEnded { _ in release() })
                Text(caption(chosen?.name)).themeText(10).multilineTextAlignment(.center).lineLimit(3)
                    .foregroundStyle(stick == .zero ? Theme.dim : Theme.accent)
            }.frame(maxWidth: .infinity)
        }
        .onChange(of: armed) { _, on in if !on { release() } }
        .onChange(of: peer.state) { _, state in if state != "live" { armed = false; release() } }
        .onDisappear { armed = false; release() }
    }

    private func release() {
        stick = .zero
        peer.drive(target: nil)
    }

    private func caption(_ name: String?) -> String {
        guard let name else { return "Waiting for a robot to join this scene." }
        if peer.state != "live" { return "Connect to Home to drive." }
        if !armed { return "Switch Arm on to drive \(name)." }
        return stick == .zero ? "Hold the stick to drive \(name). Let go to stop."
                              : "Driving \(name). Let go to stop."
    }
}

/// A device whose point of view can be shown: this phone, another phone, or a robot.
struct PovDevice: Identifiable, Equatable {
    let id: String
    let name: String
    let symbol: String
    let asleep: Bool
    /// Camera pose in Home's frame, looking along its own -Z with +Y up on screen.
    let transform: simd_float4x4
    /// False for a device Home knows about that has no pose in this scene yet.
    var located = true
    var detail: String? = nil
    /// Home's own, slightly delayed, record of this phone.
    var isSelf = false
    /// For a robot: its base pose in Home's frame, and whether that base is Z up
    /// with +X forward (Home's robot labels) or Y up with -Z forward (runner poses).
    var chassis: simd_float4x4? = nil
    var chassisZUp = false

    /// Phone camera poses use the sensor's landscape axes. Turn the view a
    /// quarter about the optical axis so a phone held upright looks upright.
    static func upright(_ camera: simd_float4x4) -> simd_float4x4 {
        camera * simd_float4x4(simd_quatf(angle: .pi / 2, axis: SIMD3(0, 0, 1)))
    }
}

/// The scene rendered from a chosen device's pose.
struct PovSceneView: UIViewRepresentable {
    let scene: SCNScene
    let transform: simd_float4x4
    func makeCoordinator() -> SCNNode {
        let node = SCNNode()
        node.camera = SCNCamera()
        node.camera?.fieldOfView = 68
        node.camera?.zNear = 0.05
        node.camera?.zFar = 60
        return node
    }
    func makeUIView(context: Context) -> SCNView {
        let view = SCNView()
        view.autoenablesDefaultLighting = true
        view.scene = scene
        view.backgroundColor = Theme.uiWell
        view.antialiasingMode = .multisampling2X
        view.rendersContinuously = false
        scene.rootNode.addChildNode(context.coordinator)
        context.coordinator.simdTransform = transform
        view.pointOfView = context.coordinator
        return view
    }
    func updateUIView(_ view: SCNView, context: Context) {
        SCNTransaction.begin(); SCNTransaction.animationDuration = 0.2
        context.coordinator.simdTransform = transform
        SCNTransaction.commit()
    }
    static func dismantleUIView(_ view: SCNView, coordinator: SCNNode) { coordinator.removeFromParentNode() }
}

/// Device list on the left; the selected device's view of the map on the right.
struct PovPanel: View {
    @ObservedObject var controller: ViewerController
    @Binding var selection: String?
    var body: some View {
        let devices = controller.devices
        let chosen = devices.first { $0.id == selection && $0.located } ?? devices.first { $0.located }
        HStack(spacing: 8) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(devices) { device in
                        Button { selection = device.id } label: {
                            HStack(spacing: 6) {
                                Image(systemName: device.asleep ? "moon.zzz.fill" : device.symbol).frame(width: 18)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(device.name + (device.asleep ? " · asleep" : "")).themeLabel(10).lineLimit(2)
                                    if !device.located {
                                        Text(device.detail ?? "no position yet").themeText(9).foregroundStyle(Theme.dim).lineLimit(2)
                                    }
                                }
                                Spacer(minLength: 0)
                            }
                            .opacity(device.located ? 1 : 0.55)
                            .themeRow(selected: device.id == chosen?.id)
                        }.buttonStyle(.plain).disabled(!device.located)
                    }
                    if devices.isEmpty {
                        Text("No devices yet. Align this phone or wait for others.").themeText(10).foregroundStyle(Theme.dim)
                    }
                }
            }.frame(width: 118)
            ZStack(alignment: .bottomLeading) {
                if let chosen {
                    PovSceneView(scene: controller.povScene, transform: chosen.transform)
                    Brackets(arm: 14).stroke(Theme.ink.opacity(0.7), style: StrokeStyle(lineWidth: 2.5, lineCap: .round)).padding(10).allowsHitTesting(false)
                    Text(chosen.name + (chosen.asleep ? " · asleep, last view" : "")).themeLabel(9, .semibold)
                        .themeTag().padding(6)
                } else {
                    Text("Pick a device").themeLabel(10).foregroundStyle(Theme.dim)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }.themeWell()
        }
    }
}

/// Home's scene for the mini map: every voxel with its color, Portals, notes.
/// Home is Y up; +X is right and -Z is up on screen in the default top-down view.
struct MiniMapData: Equatable {
    struct Cell: Equatable { let p: SIMD3<Float>; let rgb: [Float]? }
    var cells = [Cell]()
    var cellSize: Float = 0.1
    var portals = [SIMD3<Float>]()
    var notes = [SIMD3<Float>]()
    /// Semantic volumes: upright boxes with a bottom-centred position and a yaw about +Y.
    var volumes = [SemanticVolume]()
    /// Height of the ground grid: the underside of the lowest voxel.
    var groundY: Float = 0

    init() {}
    init(_ map: PortalMap) {
        let size = map.render?.voxel_size ?? 0.1
        cellSize = size.isFinite && size > 0 ? size : 0.1
        cells = (map.render?.voxels ?? []).compactMap { voxel in
            guard voxel.center.count == 3, voxel.center.allSatisfy(\.isFinite) else { return nil }
            return Cell(p: SIMD3(voxel.center[0], voxel.center[1], voxel.center[2]),
                        rgb: voxel.linear_rgb?.count == 3 ? voxel.linear_rgb : nil)
        }
        portals = map.scenegraph.anchors.values.map {
            let t = $0.pose_in_map.matrix.columns.3
            return SIMD3(t.x, t.y, t.z)
        }
        notes = (map.render?.notes ?? []).compactMap {
            $0.position.count == 3 ? SIMD3($0.position[0], $0.position[1], $0.position[2]) : nil
        }
        volumes = (map.render?.volumes ?? []).filter(\.valid)
        groundY = cells.map(\.p.y).min().map { $0 - cellSize / 2 } ?? portals.map(\.y).min() ?? 0
    }
}
struct MiniMapPhone: Equatable { let position: SIMD3<Float>; let heading: SIMD2<Float> }
/// A robot on the mini map: its chassis pose in Home's frame.
struct MiniMapRobot: Equatable {
    let name: String
    let transform: simd_float4x4
    /// True when the base is Z up and drives along +X; otherwise Y up, driving along -Z.
    var zUp = false
    /// The chassis is drawn as its 0.35 m by 0.5 m footprint.
    static let halfWidth: Float = 0.175, halfLength: Float = 0.25
    var position: SIMD3<Float> { SIMD3(transform.columns.3.x, transform.columns.3.y, transform.columns.3.z) }
    /// Footprint corners, front pair first, then the nose point used for the heading tick.
    var outline: [SIMD3<Float>] {
        [(-Self.halfWidth, -Self.halfLength), (Self.halfWidth, -Self.halfLength),
         (Self.halfWidth, Self.halfLength), (-Self.halfWidth, Self.halfLength), (0, -Self.halfLength * 2.2)].map { x, z in
            // (x, z) is across and behind the nose in the Y-up convention.
            let p = transform * (zUp ? SIMD4(-z, -x, 0, 1) : SIMD4(x, 0, z, 1))
            return SIMD3(p.x, p.y, p.z)
        }
    }
}

/// What this phone knows about a tapped device: who it is, how it is, and where it was last seen.
struct DeviceCard: View {
    let device: TrackedDevice
    /// This phone's position, for the distance line.
    let from: SIMD3<Float>?

    static func age(_ seconds: TimeInterval) -> String {
        let s = max(0, Int(seconds))
        return s < 60 ? "\(s) s" : s < 3600 ? "\(s / 60) min" : "\(s / 3600) h \(s % 3600 / 60) min"
    }
    private func state(_ now: Date) -> String {
        switch device.state {
        case .live: return "Live"
        case .asleep: return "Asleep zzz"
        case .lost: return "Lost zzz · last seen " + Self.age(now.timeIntervalSince(device.lastSeen)) + " ago"
        }
    }
    var body: some View {
        // The "last seen" line counts up on its own.
        TimelineView(.periodic(from: .now, by: 1)) { timeline in
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 5) {
                    Image(systemName: device.kind == .phone ? "iphone" : "car.side").font(.system(size: 11))
                    Text(device.name).font(Theme.font(11, .semibold))
                }.foregroundStyle(Theme.ink)
                Text(state(timeline.date)).font(Theme.font(10))
                    .foregroundStyle(device.state == .live ? Theme.ink : Theme.accent)
                let p = device.position
                Text(String(format: "x %.2f  y %.2f  z %.2f m", p.x, p.y, p.z)).font(Theme.font(10)).foregroundStyle(Theme.dim)
                if let from {
                    Text(String(format: "%.1f m from this phone", simd_distance(from, p))).font(Theme.font(10)).foregroundStyle(Theme.dim)
                }
                Text(String(format: "Trail %.1f m · ", device.travelled) + String(device.id.suffix(6)))
                    .font(Theme.font(10)).foregroundStyle(Theme.dim)
            }
            .themeBubble()
            .accessibilityElement(children: .combine)
        }
    }
}

/// Standard viewport controls: one finger orbits, two fingers truck, pinch
/// dollies, double-tap resets.
struct MiniMapGestures: UIViewRepresentable {
    var onOrbit: (CGSize) -> Void
    var onTruck: (CGSize) -> Void
    var onZoom: (CGFloat) -> Void
    var onReset: () -> Void
    var onTap: (CGPoint) -> Void = { _ in }
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeUIView(context: Context) -> UIView {
        let view = UIView()
        view.backgroundColor = .clear
        let orbit = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.orbit(_:)))
        orbit.maximumNumberOfTouches = 1
        let truck = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.truck(_:)))
        truck.minimumNumberOfTouches = 2; truck.maximumNumberOfTouches = 2
        let pinch = UIPinchGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.pinch(_:)))
        let reset = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.reset))
        reset.numberOfTapsRequired = 2
        // One tap asks about whatever is under the finger; it waits for a second tap not to come.
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tap(_:)))
        tap.require(toFail: reset)
        truck.delegate = context.coordinator; pinch.delegate = context.coordinator
        [orbit, truck, pinch, reset, tap].forEach(view.addGestureRecognizer)
        return view
    }
    func updateUIView(_ view: UIView, context: Context) { context.coordinator.parent = self }
    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        var parent: MiniMapGestures
        init(_ parent: MiniMapGestures) { self.parent = parent }
        @objc func orbit(_ g: UIPanGestureRecognizer) {
            let t = g.translation(in: g.view); g.setTranslation(.zero, in: g.view)
            parent.onOrbit(CGSize(width: t.x, height: t.y))
        }
        @objc func truck(_ g: UIPanGestureRecognizer) {
            let t = g.translation(in: g.view); g.setTranslation(.zero, in: g.view)
            parent.onTruck(CGSize(width: t.x, height: t.y))
        }
        @objc func pinch(_ g: UIPinchGestureRecognizer) { parent.onZoom(g.scale); g.scale = 1 }
        @objc func reset() { parent.onReset() }
        @objc func tap(_ g: UITapGestureRecognizer) { parent.onTap(g.location(in: g.view)) }
        // Two fingers may truck and dolly in the same motion.
        func gestureRecognizer(_ a: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith b: UIGestureRecognizer) -> Bool { true }
    }
}

/// An outline for a model in AR: an enlarged shell in the accent colour drawn over
/// everything, then the model itself on top. Where something hides the model, the
/// shell still shows, so the robot can be found behind the map or a wall.
///
/// A full-screen SCNTechnique cannot do this here: ARKit's person occlusion owns
/// the AR view's technique.
enum RobotGlow {
    private static let shell: SCNMaterial = {
        let material = SCNMaterial()
        material.lightingModel = .constant
        material.diffuse.contents = Theme.uiAccent
        material.cullMode = .front
        material.readsFromDepthBuffer = false
        material.writesToDepthBuffer = false
        // Push each vertex out along its normal by a fraction of its distance from the
        // camera, so the rim is about the same width on screen near or far.
        material.shaderModifiers = [.geometry: """
            float4 viewPosition = scn_node.modelViewTransform * _geometry.position;
            float3 viewNormal = normalize((scn_node.modelViewTransform * float4(_geometry.normal, 0.0)).xyz);
            viewPosition.xyz += viewNormal * (0.012 * -viewPosition.z);
            _geometry.position = scn_node.inverseModelViewTransform * viewPosition;
            """]
        return material
    }()
    static func outlined(_ model: SCNNode) -> SCNNode {
        let rim = model.clone()
        rim.enumerateHierarchy { node, _ in
            guard let geometry = node.geometry?.copy() as? SCNGeometry else { return }
            geometry.materials = Array(repeating: shell, count: max(1, geometry.materials.count))
            node.geometry = geometry
            node.renderingOrder = 100
        }
        model.enumerateHierarchy { node, _ in node.renderingOrder = 101 }
        let holder = SCNNode()
        holder.addChildNode(rim); holder.addChildNode(model)
        return holder
    }
}

/// The robot's 3D model drawn over the mini map. Its camera is orthographic and
/// copies the map's own projection, so the model orbits, trucks and zooms with it.
struct RobotMapLayer: UIViewRepresentable {
    let robots: [MiniMapRobot]
    let center: SIMD3<Float>
    /// Points per metre.
    let scale: Float
    let yaw: Float
    let pitch: Float
    let size: CGSize

    /// The rigged Bracketbot, loaded once off the main thread. Authored Z up in metres.
    private(set) static var template: SCNNode?
    private static var waiting = [() -> Void]()
    private static var loading = false
    static func whenLoaded(_ ready: @escaping () -> Void) {
        if template != nil { ready(); return }
        waiting.append(ready)
        guard !loading else { return }
        loading = true
        DispatchQueue.global(qos: .userInitiated).async {
            var model: SCNNode?
            if let url = Bundle.main.url(forResource: "bracketbot_0184_rigged", withExtension: "usdz"),
               let scene = try? SCNScene(url: url, options: [.checkConsistency: false]) {
                let loaded = scene.rootNode.childNodes.first
                loaded?.childNode(withName: "env_light", recursively: true)?.removeFromParentNode()
                // The model faces a quarter turn off the base's heading: turn it about the vertical.
                let turned = SCNNode()
                turned.simdOrientation = simd_quatf(angle: .pi / 2, axis: SIMD3(0, 0, 1))
                if let loaded { turned.addChildNode(loaded); model = turned }
            }
            DispatchQueue.main.async {
                template = model
                let pending = waiting; waiting = []
                if model != nil { pending.forEach { $0() } }
            }
        }
    }

    final class Coordinator {
        let scene = SCNScene()
        let camera = SCNNode()
        var nodes = [String: SCNNode]()
        var robots = [MiniMapRobot]()
        func sync() {
            guard let template = RobotMapLayer.template else { return }
            for (name, node) in nodes where !robots.contains(where: { $0.name == name }) {
                node.removeFromParentNode(); nodes[name] = nil
            }
            for robot in robots {
                let node = nodes[robot.name] ?? {
                    let holder = SCNNode()
                    let upright = SCNNode()
                    // The model is Z up. A Y-up pose needs it stood up; a Z-up base pose already is.
                    if !robot.zUp { upright.simdOrientation = simd_quatf(angle: -.pi / 2, axis: SIMD3(1, 0, 0)) }
                    upright.addChildNode(template.clone())
                    holder.addChildNode(upright)
                    scene.rootNode.addChildNode(holder)
                    nodes[robot.name] = holder
                    return holder
                }()
                node.simdTransform = robot.transform
            }
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> SCNView {
        let view = SCNView()
        view.scene = context.coordinator.scene
        view.backgroundColor = .clear
        view.isOpaque = false
        view.isUserInteractionEnabled = false
        view.autoenablesDefaultLighting = true
        view.antialiasingMode = .multisampling4X
        view.rendersContinuously = false
        let camera = SCNCamera()
        camera.usesOrthographicProjection = true
        camera.zNear = 0.1; camera.zFar = 400
        context.coordinator.camera.camera = camera
        context.coordinator.scene.rootNode.addChildNode(context.coordinator.camera)
        view.pointOfView = context.coordinator.camera
        let coordinator = context.coordinator
        RobotMapLayer.whenLoaded { [weak coordinator] in coordinator?.sync() }
        return view
    }
    func updateUIView(_ view: SCNView, context: Context) {
        context.coordinator.robots = robots
        context.coordinator.sync()
        // The map's projection as a camera: screen right, screen up, and the axis toward the viewer.
        let right = SIMD3(cos(yaw), 0, -sin(yaw))
        let up = SIMD3(-sin(yaw) * sin(pitch), cos(pitch), -cos(yaw) * sin(pitch))
        let toward = SIMD3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch))
        let eye = center + toward * 200
        context.coordinator.camera.simdTransform = simd_float4x4(
            SIMD4(right, 0), SIMD4(up, 0), SIMD4(toward, 0), SIMD4(eye, 1))
        // Half the visible height in metres.
        context.coordinator.camera.camera?.orthographicScale = Double(size.height) / 2 / Double(max(scale, 0.001))
    }
}

struct MiniMapView: View {
    let map: MiniMapData
    let phone: MiniMapPhone?
    var robots = [MiniMapRobot]()
    /// Every other phone and robot seen in this scene, at its last known place, with its trail.
    var tracked = [TrackedDevice]()
    /// The device last tapped, described in a card until the map is tapped elsewhere.
    @State private var queried: String?
    /// One switch for the robot trails here and in AR.
    var showTrails: Binding<Bool> = .constant(true)
    var clearTrails: () -> Void = {}
    /// Offset of the orbit pivot from the scene's center, in Home metres.
    @State private var pan = SIMD3<Float>(0, 0, 0)
    @State private var yaw: Float = 0
    /// 90 degrees looks straight down; lower values tilt toward the horizon.
    @State private var pitch: Float = .pi / 2
    @State private var zoom: Float = 1
    @AppStorage("showVolumesOnMap") private var showVolumes = true
    /// Robots as the 3D model, or as a flat footprint icon.
    @AppStorage("showRobotModelOnMap") private var showRobotModel = true

    /// The eight corners of a volume in Home metres: base ring, then top ring.
    static func corners(_ volume: SemanticVolume) -> [SIMD3<Float>] {
        let base = SIMD3(volume.position[0], volume.position[1], volume.position[2])
        let (w, h, d) = (volume.dimensions[0] / 2, volume.dimensions[1], volume.dimensions[2] / 2)
        let (cosine, sine) = (cos(volume.yaw), sin(volume.yaw))
        let ring = [(-w, -d), (w, -d), (w, d), (-w, d)].map { x, z in
            // Yaw about +Y, the same turn SceneKit applies to the AR box.
            base + SIMD3(x * cosine + z * sine, 0, -x * sine + z * cosine)
        }
        return ring + ring.map { $0 + SIMD3(0, h, 0) }
    }

    var body: some View {
        GeometryReader { geometry in
            let size = geometry.size
            let view = projection(size)
            ZStack(alignment: .topTrailing) {
                Canvas { context, _ in draw(&context, size: size, view: view) }
                if !robots.isEmpty && showRobotModel {
                    RobotMapLayer(robots: robots, center: view.center, scale: view.scale, yaw: view.yaw,
                                  pitch: view.pitch, size: size)
                        .allowsHitTesting(false)
                }
                Canvas { context, _ in drawMarkers(&context, view: view) }.allowsHitTesting(false)
                MiniMapGestures(
                    onOrbit: { d in
                        // Orbit about the pivot, which stays at the center of the view.
                        yaw += Float(d.width) * 0.008
                        pitch = max(0.12, min(.pi / 2, pitch - Float(d.height) * 0.008))
                    },
                    onTruck: { d in
                        // Slide the pivot along the camera's own right and up axes.
                        let sx = Float(d.width) / view.scale, sy = Float(d.height) / view.scale
                        let turnedX = sx, turnedZ = sy * sin(pitch), up = -sy * cos(pitch)
                        pan -= SIMD3(turnedX * cos(yaw) + turnedZ * sin(yaw), up, -turnedX * sin(yaw) + turnedZ * cos(yaw))
                    },
                    onZoom: { zoom = max(0.4, min(12, zoom * Float($0))) },
                    onReset: { withAnimation { pan = .zero; yaw = 0; pitch = .pi / 2; zoom = 1 } },
                    onTap: { point in
                        let near = tracked.map { ($0.id, view.place($0.position).point) }
                            .map { ($0.0, hypot($0.1.x - point.x, $0.1.y - point.y)) }
                            .filter { $0.1 < 28 }.min { $0.1 < $1.1 }
                        queried = near?.0
                    })
                VStack(spacing: 0) {
                    Button { showVolumes.toggle() } label: {
                        Image(systemName: showVolumes ? "cube.fill" : "cube").font(.system(size: 13))
                            .foregroundStyle(showVolumes ? Theme.accent : Theme.dim)
                            .themeDot()
                    }.padding(6).accessibilityLabel(showVolumes ? "Hide semantic volumes on the map" : "Show semantic volumes on the map")
                    Button { showRobotModel.toggle() } label: {
                        Image(systemName: showRobotModel ? "view.3d" : "view.2d").font(.system(size: 13))
                            .foregroundStyle(showRobotModel ? Theme.accent : Theme.dim)
                            .themeDot()
                    }.padding(.horizontal, 6).padding(.bottom, 6)
                        .accessibilityLabel(showRobotModel ? "Show robots as icons" : "Show robots as 3D models")
                    // Tap shows or hides robot trails; a long press starts them again from here.
                    Image(systemName: "point.topleft.down.to.point.bottomright.curvepath").font(.system(size: 13))
                        .foregroundStyle(showTrails.wrappedValue ? Theme.accent : Theme.dim)
                        .themeDot()
                        .padding(.horizontal, 6).padding(.bottom, 6)
                        .onTapGesture { showTrails.wrappedValue.toggle() }
                        .onLongPressGesture { clearTrails(); UIImpactFeedbackGenerator(style: .medium).impactOccurred() }
                        .accessibilityLabel(showTrails.wrappedValue ? "Hide robot trails" : "Show robot trails")
                        .accessibilityHint("Long press to clear the trails")
                        .accessibilityAddTraits(.isButton)
                    if pan != SIMD3<Float>(0, 0, 0) || yaw != 0 || pitch != .pi / 2 || zoom != 1 {
                        Button { pan = .zero; yaw = 0; pitch = .pi / 2; zoom = 1 } label: {
                            Image(systemName: "scope").font(.system(size: 13)).foregroundStyle(Theme.ink)
                                .themeDot()
                        }.padding(.horizontal, 6).accessibilityLabel("Reset map view")
                    }
                }
            }
        }
        .themeWell()
        .overlay(alignment: .bottomLeading) {
            if let device = tracked.first(where: { $0.id == queried }) {
                DeviceCard(device: device, from: phone?.position).padding(6)
            }
        }
        .accessibilityLabel("Mini map")
    }

    private struct Projection {
        let center: SIMD3<Float>, scale: Float, yaw: Float, pitch: Float, size: CGSize
        /// Orthographic: turn about Y, then tilt. Depth grows toward the viewer.
        func place(_ p: SIMD3<Float>) -> (point: CGPoint, depth: Float) {
            let d = p - center
            let x = d.x * cos(yaw) - d.z * sin(yaw), z = d.x * sin(yaw) + d.z * cos(yaw)
            let y = z * sin(pitch) - d.y * cos(pitch)
            return (CGPoint(x: CGFloat(x * scale) + size.width / 2, y: CGFloat(y * scale) + size.height / 2),
                    d.y * sin(pitch) + z * cos(pitch))
        }
    }

    private func bounds() -> (low: SIMD3<Float>, high: SIMD3<Float>)? {
        var points = map.cells.map(\.p) + map.portals + map.notes
        if showVolumes { points += map.volumes.flatMap { Self.corners($0) } }
        if let phone { points.append(phone.position) }
        points += robots.map(\.position)
        points += tracked.filter { $0.kind == .phone || $0.state == .lost }.map(\.position)
        if showTrails.wrappedValue { points += tracked.flatMap(\.trail) }
        guard var low = points.first else { return nil }
        var high = low
        for p in points { low = simd_min(low, p); high = simd_max(high, p) }
        return (low, high)
    }

    private func projection(_ size: CGSize) -> Projection {
        let (low, high) = bounds() ?? (SIMD3(repeating: -1), SIMD3(repeating: 1))
        // Fit the footprint, with at least a 2 m window so one Portal is not blown up.
        let span = SIMD2(max(high.x - low.x + 0.6, 2), max(high.z - low.z + 0.6, 2))
        let fit = min(Float(size.width) / span.x, Float(size.height) / span.y)
        let middle = (low + high) / 2
        return Projection(center: SIMD3(middle.x, map.groundY, middle.z) + pan,
                          scale: max(1, fit * zoom), yaw: yaw, pitch: pitch, size: size)
    }

    /// Drawn above everything else on the map: each device's trail, then a badge
    /// for every phone and robot at its last known place.
    private func drawMarkers(_ context: inout GraphicsContext, view: Projection) {
        if showTrails.wrappedValue {
            // Pale blue where the device started, yellow where it is now, cased in black to read on any map.
            for device in tracked where !device.trail.isEmpty {
                let points = device.trail.map { view.place($0).point } + [view.place(device.position).point]
                guard points.count > 1 else { continue }
                var whole = Path(); whole.addLines(points)
                context.stroke(whole, with: .color(.black.opacity(0.75)), style: StrokeStyle(lineWidth: 8, lineCap: .round, lineJoin: .round))
                for i in 1..<points.count {
                    var segment = Path(); segment.move(to: points[i - 1]); segment.addLine(to: points[i])
                    let c = DeviceTrails.color(age: Float(i) / Float(points.count - 1))
                    context.stroke(segment, with: .color(Color(red: Double(c.x), green: Double(c.y), blue: Double(c.z))),
                                   style: StrokeStyle(lineWidth: 5, lineCap: .round))
                }
            }
        }
        func badge(at point: CGPoint, lift: CGFloat = 0, symbol: String?, fill: Color, name: String, resting: Bool, chosen: Bool) {
            let strength = resting ? 0.55 : 1.0
            // A lifted badge is a pin: it stands clear of what it marks, on a stalk.
            let at = CGPoint(x: point.x, y: point.y - lift)
            if lift > 0 {
                var stalk = Path(); stalk.move(to: point); stalk.addLine(to: at)
                context.stroke(stalk, with: .color(.black.opacity(0.8)), lineWidth: 4)
                context.stroke(stalk, with: .color(.white.opacity(strength)), lineWidth: 2)
            }
            let disc = CGRect(x: at.x - 17, y: at.y - 17, width: 34, height: 34)
            context.fill(Path(ellipseIn: disc), with: .color(fill.opacity(strength)))
            context.stroke(Path(ellipseIn: disc), with: .color(.white.opacity(strength)), lineWidth: 2.5)
            context.stroke(Path(ellipseIn: disc.insetBy(dx: -1.5, dy: -1.5)), with: .color(.black.opacity(0.8 * strength)), lineWidth: 1)
            if let symbol {
                var icon = context.resolve(Image(systemName: symbol).symbolRenderingMode(.monochrome))
                icon.shading = .color(.white)
                context.draw(icon, in: CGRect(x: at.x - 9, y: at.y - 11, width: 18, height: 22))
            } else {
                // Robot face: a head with two eyes and an antenna.
                let head = CGRect(x: at.x - 9, y: at.y - 5, width: 18, height: 13)
                context.fill(Path(roundedRect: head, cornerRadius: 3), with: .color(.white))
                for dx in [-4.0, 4.0] {
                    context.fill(Path(ellipseIn: CGRect(x: at.x + dx - 2, y: at.y - 1, width: 4, height: 4)), with: .color(fill))
                }
                var antenna = Path(); antenna.move(to: CGPoint(x: at.x, y: at.y - 5)); antenna.addLine(to: CGPoint(x: at.x, y: at.y - 10))
                context.stroke(antenna, with: .color(.white), lineWidth: 2)
                context.fill(Path(ellipseIn: CGRect(x: at.x - 2.5, y: at.y - 13.5, width: 5, height: 5)), with: .color(.white))
            }
            let label = context.resolve(Text(name).font(Theme.font(11, .bold)).foregroundColor(.white))
            let measured = label.measure(in: CGSize(width: 200, height: 30))
            let plate = CGRect(x: at.x - measured.width / 2 - 4, y: disc.maxY + 3, width: measured.width + 8, height: measured.height + 2)
            context.fill(Path(plate), with: .color(.black.opacity(0.8)))
            context.draw(label, at: CGPoint(x: plate.midX, y: plate.midY))
            if resting {
                let zzz = context.resolve(Text("zzz").font(Theme.font(14, .bold)).foregroundColor(.white))
                let size = zzz.measure(in: CGSize(width: 60, height: 30))
                let bubble = CGRect(x: disc.maxX - 6, y: disc.minY - size.height + 4, width: size.width + 8, height: size.height + 2)
                context.fill(Path(roundedRect: bubble, cornerRadius: 4), with: .color(Theme.accent))
                context.draw(zzz, at: CGPoint(x: bubble.midX, y: bubble.midY))
            }
            if chosen {
                context.stroke(Path(ellipseIn: disc.insetBy(dx: -6, dy: -6)), with: .color(Theme.accent), lineWidth: 2.5)
            }
        }
        if let phone {
            badge(at: view.place(phone.position).point, symbol: "iphone", fill: Theme.accent, name: "You", resting: false, chosen: false)
        }
        for device in tracked {
            // A robot's badge is a pin, so it stands clear of its model and of a phone beside it.
            badge(at: view.place(device.position).point, lift: device.kind == .robot ? 52 : 0,
                  symbol: device.kind == .phone ? "iphone" : nil,
                  fill: device.kind == .phone ? Color(red: 0.1, green: 0.45, blue: 0.95) : Color(red: 0.05, green: 0.6, blue: 0.3),
                  name: device.name, resting: device.state != .live, chosen: device.id == queried)
        }
    }

    private func draw(_ context: inout GraphicsContext, size: CGSize, view: Projection) {
        // Registration marks in the corners of the viewport.
        for corner in [CGPoint(x: 12, y: 12), CGPoint(x: 12, y: size.height - 12), CGPoint(x: size.width - 12, y: size.height - 12)] {
            var mark = Path()
            mark.move(to: CGPoint(x: corner.x - 4, y: corner.y)); mark.addLine(to: CGPoint(x: corner.x + 4, y: corner.y))
            mark.move(to: CGPoint(x: corner.x, y: corner.y - 4)); mark.addLine(to: CGPoint(x: corner.x, y: corner.y + 4))
            context.stroke(mark, with: .color(Theme.dim), lineWidth: 0.6)
        }
        guard let (low, high) = bounds() else {
            context.draw(Text("No map yet. Connect and scan a Portal.").font(Theme.font(10)).foregroundColor(Theme.dim),
                         at: CGPoint(x: size.width / 2, y: size.height / 2))
            return
        }
        context.draw(Text("1 m grid").font(Theme.font(9)).foregroundColor(Theme.dim),
                     at: CGPoint(x: 22, y: size.height - 12), anchor: .leading)
        // Ground grid at the lowest level: 1 m lines, bolder every 5 m.
        let x0 = (low.x - 1).rounded(.down), x1 = (high.x + 1).rounded(.up)
        let z0 = (low.z - 1).rounded(.down), z1 = (high.z + 1).rounded(.up)
        if x1 - x0 <= 200, z1 - z0 <= 200 {
            func line(_ a: SIMD3<Float>, _ b: SIMD3<Float>, bold: Bool) {
                var path = Path(); path.move(to: view.place(a).point); path.addLine(to: view.place(b).point)
                context.stroke(path, with: .color(Theme.ink.opacity(bold ? 0.3 : 0.12)), lineWidth: bold ? 0.8 : 0.5)
            }
            var x = x0
            while x <= x1 { line(SIMD3(x, map.groundY, z0), SIMD3(x, map.groundY, z1), bold: Int(x) % 5 == 0); x += 1 }
            var z = z0
            while z <= z1 { line(SIMD3(x0, map.groundY, z), SIMD3(x1, map.groundY, z), bold: Int(z) % 5 == 0); z += 1 }
        }
        // Painter's order: farthest first, so nearer and higher voxels cover them.
        let placed = map.cells.map { (view.place($0.p), $0) }.sorted { $0.0.depth < $1.0.depth }
        // Voxels are drawn as separate dots on the grey well.
        let cell = CGFloat(map.cellSize * view.scale)
        let side = cell > 3 ? cell * 0.8 : cell + 0.5
        let heightRange = max(0.5, high.y - map.groundY)
        for (at, cell) in placed {
            // Uncolored voxels are a pale relief, lighter with height.
            let shade = 0.56 + 0.42 * Double(max(0, min(1, (cell.p.y - map.groundY) / heightRange)))
            let color: Color
            if let rgb = cell.rgb {
                // A voxel's own color, softened a little toward the relief tone.
                let grey = pow(shade, 2.2)
                func mix(_ c: Float) -> Double { 0.8 * Double(c) + 0.2 * grey }
                color = Color(.sRGBLinear, red: mix(rgb[0]), green: mix(rgb[1]), blue: mix(rgb[2]))
            } else {
                color = Color(white: shade)
            }
            context.fill(Path(CGRect(x: at.point.x - side / 2, y: at.point.y - side / 2, width: side, height: side)), with: .color(color))
        }
        if showVolumes {
            // Semantic volumes: a tinted footprint, the box edges, and its name.
            let tint = Theme.accent
            for volume in map.volumes {
                let c = Self.corners(volume).map { view.place($0).point }
                func ring(_ i: [Int]) -> Path {
                    var path = Path(); path.move(to: c[i[0]])
                    i.dropFirst().forEach { path.addLine(to: c[$0]) }; path.closeSubpath()
                    return path
                }
                context.fill(ring([0, 1, 2, 3]), with: .color(tint.opacity(0.18)))
                context.stroke(ring([0, 1, 2, 3]), with: .color(tint), lineWidth: 1)
                context.stroke(ring([4, 5, 6, 7]), with: .color(tint), lineWidth: 1)
                for i in 0..<4 {
                    var edge = Path(); edge.move(to: c[i]); edge.addLine(to: c[i + 4])
                    context.stroke(edge, with: .color(tint.opacity(0.8)), lineWidth: 0.6)
                }
                let top = CGPoint(x: (c[4].x + c[6].x) / 2, y: (c[4].y + c[6].y) / 2)
                context.draw(Text(volume.name + " · " + volume.semantic_type).font(Theme.font(9, .semibold)).foregroundColor(Theme.ink), at: top)
            }
        }
        for note in map.notes {
            let at = view.place(note).point
            let dot = Path(ellipseIn: CGRect(x: at.x - 4, y: at.y - 4, width: 8, height: 8))
            context.fill(dot, with: .color(Theme.paper)); context.stroke(dot, with: .color(Theme.ink), lineWidth: 1)
        }
        for portal in map.portals {
            let at = view.place(portal).point
            let square = Path(CGRect(x: at.x - 5, y: at.y - 5, width: 10, height: 10))
            context.fill(square, with: .color(Theme.ink)); context.stroke(square, with: .color(Theme.paper), lineWidth: 1)
        }
        // A robot Home no longer reports stays where it was last seen, faded.
        let ghosts = tracked.filter { $0.kind == .robot && $0.state == .lost }
            .map { MiniMapRobot(name: $0.name, transform: $0.pose, zUp: true) }
        for (robot, strength) in robots.map({ ($0, 1.0) }) + ghosts.map({ ($0, 0.3) }) {
            // Under the 3D model: the chassis footprint, a tick out of its nose, and its name.
            let c = robot.outline.map { view.place($0).point }
            var body = Path(); body.move(to: c[0]); c[1...3].forEach { body.addLine(to: $0) }; body.closeSubpath()
            if !showRobotModel || strength < 1 { context.fill(body, with: .color(Theme.accent.opacity(0.35 * strength))) }
            context.stroke(body, with: .color(Theme.accent.opacity(strength)), lineWidth: showRobotModel ? 1 : 1.5)
            var nose = Path()
            nose.move(to: CGPoint(x: (c[0].x + c[1].x) / 2, y: (c[0].y + c[1].y) / 2)); nose.addLine(to: c[4])
            context.stroke(nose, with: .color(Theme.accent.opacity(strength)), lineWidth: 1.5)
            let at = view.place(robot.position).point
            let reach = max(14, c[0..<4].map { hypot($0.x - at.x, $0.y - at.y) }.max() ?? 14) + 5
            let reticle = CGRect(x: at.x - reach, y: at.y - reach, width: reach * 2, height: reach * 2)
            context.stroke(Brackets(arm: 6).path(in: reticle), with: .color(Theme.ink.opacity(strength)), lineWidth: 1)
            context.draw(Text(robot.name).font(Theme.font(9, .semibold)).foregroundColor(Theme.ink.opacity(strength)),
                         at: CGPoint(x: reticle.midX, y: reticle.minY - 3), anchor: .bottom)
        }
        if let phone {
            let at = view.place(phone.position).point
            // A drop line shows the phone's height once the view is tilted.
            let foot = view.place(SIMD3(phone.position.x, map.groundY, phone.position.z)).point
            var drop = Path(); drop.move(to: at); drop.addLine(to: foot)
            context.stroke(drop, with: .color(Theme.ink.opacity(0.6)), style: StrokeStyle(lineWidth: 0.8, dash: [3, 3]))
            // Crosshair and reticle on this phone, with its heading from Home's -Z.
            var cross = Path()
            cross.move(to: CGPoint(x: 0, y: at.y)); cross.addLine(to: CGPoint(x: size.width, y: at.y))
            cross.move(to: CGPoint(x: at.x, y: 0)); cross.addLine(to: CGPoint(x: at.x, y: size.height))
            context.stroke(cross, with: .color(Theme.ink.opacity(0.22)), lineWidth: 0.5)
            let reticle = CGRect(x: at.x - 24, y: at.y - 24, width: 48, height: 48)
            context.stroke(Brackets(arm: 9).path(in: reticle), with: .color(Theme.ink), lineWidth: 1)
            var bearing = atan2(phone.heading.x, -phone.heading.y) * 180 / .pi
            if bearing < 0 { bearing += 360 }
            if bearing.isFinite {
                context.draw(Text(String(format: "%03.0f°", bearing)).font(Theme.font(9, .semibold)).foregroundColor(Theme.ink),
                             at: CGPoint(x: reticle.maxX + 4, y: reticle.maxY), anchor: .bottomLeading)
            }
            let ahead = view.place(phone.position + SIMD3(phone.heading.x, 0, phone.heading.y)).point
            var forward = CGVector(dx: ahead.x - at.x, dy: ahead.y - at.y)
            let length = max(0.001, hypot(forward.dx, forward.dy))
            forward = CGVector(dx: forward.dx / length, dy: forward.dy / length)
            let right = CGVector(dx: -forward.dy, dy: forward.dx)
            var arrow = Path()
            arrow.move(to: CGPoint(x: at.x + forward.dx * 14, y: at.y + forward.dy * 14))
            arrow.addLine(to: CGPoint(x: at.x - forward.dx * 8 + right.dx * 8, y: at.y - forward.dy * 8 + right.dy * 8))
            arrow.addLine(to: CGPoint(x: at.x - forward.dx * 8 - right.dx * 8, y: at.y - forward.dy * 8 - right.dy * 8))
            arrow.closeSubpath()
            context.fill(arrow, with: .color(Theme.accent)); context.stroke(arrow, with: .color(Theme.paper), lineWidth: 1)
        }
    }
}

struct CameraView: UIViewRepresentable {
    let controller: ViewerController
    func makeUIView(context: Context) -> ARSCNView { controller.start(); return controller.view }
    func updateUIView(_ uiView: ARSCNView, context: Context) {}
}

final class ViewerController: NSObject, ObservableObject, ARSessionDelegate {
    let volumes = VolumeAuthoring()
    /// Saved semantic volumes in the AR view and the POV tab. A draft being
    /// authored always stays visible.
    @Published var showVolumesInAR = UserDefaults.standard.object(forKey: "arVolumesVisible") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showVolumesInAR, forKey: "arVolumesVisible"); applyVolumeVisibility() }
    }
    private func applyVolumeVisibility() {
        // The authoring controller owns the AR nodes and re-shows them on every
        // redraw, so it carries the flag; the POV copy is plain nodes.
        volumes.hideSaved = !showVolumesInAR
        volumes.redraw()
        for node in povRoot.childNodes where node.name?.hasPrefix("semantic-volume:") == true {
            node.isHidden = !showVolumesInAR
        }
    }
    func beginVolume() {
        guard let map else { volumes.message = "Connect to Home first."; return }
        volumes.begin(frame: map.scenegraph.map.frame.id)
    }
    /// A circle on whatever surface the middle of the screen is looking at: floor, table, wall or screen.
    func beginSurface() {
        guard let map else { volumes.message = "Connect to Home first."; return }
        guard volumes.draft == nil else { return }
        let centre = CGPoint(x: view.bounds.midX, y: view.bounds.midY)
        // A plane ARKit has already mapped, else its best guess at the surface under the point.
        let found = [ARRaycastQuery.Target.existingPlaneGeometry, .estimatedPlane].lazy.compactMap { target in
            self.view.raycastQuery(from: centre, allowing: target, alignment: .any)
                .flatMap { self.view.session.raycast($0).first }
        }.first
        guard let found else {
            volumes.message = "Aim at a floor, table, wall or screen and hold still for a moment."; return
        }
        let inverse = root.simdTransform.inverse
        let hit = inverse * found.worldTransform.columns.3
        // A raycast result's +Y is the surface normal.
        let normal = inverse * found.worldTransform.columns.1
        // The circle starts no wider than the surface ARKit has mapped.
        let extent = (found.anchor as? ARPlaneAnchor)?.planeExtent
        volumes.beginSurface(frame: map.scenegraph.map.frame.id, at: SIMD3(hit.x, hit.y, hit.z),
                             normal: simd_normalize(SIMD3(normal.x, normal.y, normal.z)),
                             within: extent.map { min($0.width, $0.height) })
    }
    /// The point being named. It floats in front of the camera, shedding pink dust, until it is saved.
    private var floating: SCNNode?
    /// How far in front of the camera a new point floats: 8 inches.
    private static let reach: Float = 0.2
    /// Where a new point floats, in the axes of the upright screen: 8 inches away, seen just above the name card.
    private func aim(_ camera: ARCamera) -> SIMD3<Float> {
        let lens = camera.projectionMatrix(for: .portrait, viewportSize: view.bounds.size, zNear: 0.01, zFar: 100)
        // Screen centre across; 40% of the way down, which is 0.2 above the middle on a scale of -1 to 1.
        return simd_normalize(SIMD3(0, 0.2 / lens.columns.1.y, -1)) * Self.reach
    }
    /// That spot in the room right now.
    private static func ahead(_ aim: SIMD3<Float>, of camera: ARCamera) -> SIMD3<Float> {
        let spot = camera.viewMatrix(for: .portrait).inverse * SIMD4(aim.x, aim.y, aim.z, 1)
        return SIMD3(spot.x, spot.y, spot.z)
    }
    /// A note that follows the phone, 8 inches in front of it, until it is saved.
    func beginPoint() {
        guard volumes.draft == nil, placement == nil else { return }
        guard aligned, let map, let frame = view.session.currentFrame, case .normal = frame.camera.trackingState else {
            volumes.message = "Scan a Portal before placing a point."; return
        }
        let aim = aim(frame.camera)
        let bubble = Self.bubble("")
        bubble.name = "annotation-preview"
        root.addChildNode(bubble)
        bubble.simdWorldPosition = Self.ahead(aim, of: frame.camera)
        // Moved on every drawn frame, not every camera frame, so it glides. It trails the phone
        // a little and catches up in about a tenth of a second.
        var last = CACurrentMediaTime()
        let follow = SCNTransformConstraint.positionConstraint(inWorldSpace: true) { [weak view] _, position in
            guard let camera = view?.session.currentFrame?.camera else { return position }
            let now = CACurrentMediaTime()
            let step = Float(min(0.1, now - last))
            last = now
            let here = SIMD3(position.x, position.y, position.z)
            let next = here + (Self.ahead(aim, of: camera) - here) * (1 - exp(-step * 12))
            return SCNVector3(next.x, next.y, next.z)
        }
        bubble.constraints = (bubble.constraints ?? []) + [follow]
        bubble.addParticleSystem(Self.dust())
        floating = bubble
        let eye = frame.camera.transform.columns.3
        let at = bubble.simdPosition, from = root.simdConvertPosition(SIMD3(eye.x, eye.y, eye.z), from: nil)
        placement = NotePlacement(id: UUID().uuidString, frame: map.scenegraph.map.frame.id,
            origin: [from.x, from.y, from.z], direction: [0, 0, -1], at: [at.x, at.y, at.z])
    }
    private var floatingText = ""
    private var floatingPhoto: UIImage?
    /// Photos taken for points, by note. They stay on this phone: Home is not sent them yet.
    private var photos = [String: UIImage]()
    /// The floating point shows what is being typed.
    func namePoint(_ text: String) {
        guard let floating else { return }
        floatingText = text
        floating.childNodes.forEach { $0.removeFromParentNode() }
        Self.bubble(text, photo: floatingPhoto).childNodes.forEach { floating.addChildNode($0) }
    }
    /// What the camera sees right now, cut square.
    func cameraPhoto() -> UIImage? {
        guard let buffer = view.session.currentFrame?.capturedImage else { return nil }
        let upright = CIImage(cvPixelBuffer: buffer).oriented(.right)
        let side = min(upright.extent.width, upright.extent.height)
        let square = upright.cropped(to: CGRect(x: upright.extent.midX - side / 2, y: upright.extent.midY - side / 2,
                                                width: side, height: side))
            .transformed(by: CGAffineTransform(scaleX: 720 / side, y: 720 / side))
        return CIContext().createCGImage(square, from: square.extent).map { UIImage(cgImage: $0) }
    }
    /// The picture shown above the floating point: from the camera or the photo library. Nil clears it.
    func setPointPhoto(_ photo: UIImage?) {
        floatingPhoto = photo
        namePoint(floatingText)
    }
    /// Stop the point exactly where it is seen. Nil if tracking has been lost.
    func lockPoint(_ placement: NotePlacement) -> NotePlacement? {
        guard let floating, aligned, let frame = view.session.currentFrame,
              case .normal = frame.camera.trackingState else { return nil }
        let eye = frame.camera.transform.columns.3
        let at = root.simdConvertPosition(floating.presentation.simdWorldPosition, from: nil)
        let from = root.simdConvertPosition(SIMD3(eye.x, eye.y, eye.z), from: nil)
        floating.constraints = floating.constraints?.filter { $0 is SCNBillboardConstraint }
        floating.simdPosition = at
        floating.particleSystems?.forEach { $0.birthRate = 0 }
        // Home wants to know where the note was placed from: the phone.
        return NotePlacement(id: placement.id, frame: placement.frame, origin: [from.x, from.y, from.z],
                             direction: [at.x - from.x, at.y - from.y, at.z - from.z], at: [at.x, at.y, at.z])
    }
    /// Points saved on this phone that Home has not sent back yet. They are drawn here until it does.
    private var unsynced = [(id: String, frame: String, text: String, position: [Float])]()
    /// The locked point stays where it is, whether or not Home can be reached right now.
    func keepPoint(_ placement: NotePlacement, text: String) {
        guard let at = placement.at else { return }
        unsynced.append((placement.id, placement.frame, text, at))
        photos[placement.id] = floatingPhoto
        // No longer the preview, so closing the editor leaves it in the room.
        floating?.name = nil
        floating = nil
    }
    /// Pink dust left in the room behind a point that is still being carried.
    private static func dust() -> SCNParticleSystem {
        let dust = SCNParticleSystem()
        dust.particleImage = UIGraphicsImageRenderer(size: CGSize(width: 32, height: 32)).image { context in
            let colors = [UIColor.white.cgColor, UIColor.white.withAlphaComponent(0).cgColor] as CFArray
            let glow = CGGradient(colorsSpace: nil, colors: colors, locations: [0, 1])!
            context.cgContext.drawRadialGradient(glow, startCenter: CGPoint(x: 16, y: 16), startRadius: 0,
                                                 endCenter: CGPoint(x: 16, y: 16), endRadius: 16, options: [])
        }
        dust.particleColor = Theme.uiAccent
        dust.particleColorVariation = SCNVector4(0.04, 0.2, 0.1, 0)
        dust.particleSize = 0.004; dust.particleSizeVariation = 0.003
        dust.birthRate = 70
        dust.particleLifeSpan = 1.4; dust.particleLifeSpanVariation = 0.5
        dust.emitterShape = SCNSphere(radius: 0.015)
        dust.particleVelocity = 0.015; dust.particleVelocityVariation = 0.015
        dust.spreadingAngle = 180
        dust.acceleration = SCNVector3(0, -0.03, 0)
        dust.blendMode = .additive
        dust.isLightingEnabled = false
        let fade = CAKeyframeAnimation()
        fade.values = [0, 1, 0]; fade.keyTimes = [0, 0.15, 1]
        dust.propertyControllers = [.opacity: SCNParticlePropertyController(animation: fade)]
        return dust
    }
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
    private var alignment = AlignmentFilter()
    /// A Portal missing from the scene, measured while aligned, is offered to Home.
    var onNewPortal: ((PortalReport) -> Bool)?
    private var portalCandidates = [String: (position: SIMD3<Float>, time: TimeInterval)]()
    /// The alignment as it stood when each target was last read, to measure a moved target from another one.
    private var alignedThrough = [String: (homeInAR: simd_float4x4, time: TimeInterval)]()
    private var portalReported = [String: TimeInterval]()
    @Published private(set) var miniMap = MiniMapData()
    @Published private(set) var miniPhone: MiniMapPhone?
    /// Every other phone and robot seen in this scene, each at its last known place with its path.
    @Published private(set) var trackedDevices = [TrackedDevice]()
    private var trails = DeviceTrails()
    @Published var showTrails = UserDefaults.standard.object(forKey: "robotTrailsVisible") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(showTrails, forKey: "robotTrailsVisible")
            for node in root.childNodes + povRoot.childNodes where node.name == "robot-trail" { node.isHidden = !showTrails }
        }
    }
    func clearTrails() {
        trails.clear()
        trackedDevices = trackedDevices.map {
            TrackedDevice(id: $0.id, kind: $0.kind, name: $0.name, pose: $0.pose, trail: [], state: $0.state, lastSeen: $0.lastSeen)
        }
        for node in root.childNodes + povRoot.childNodes where node.name == "robot-trail" { node.removeFromParentNode() }
    }
    /// Devices for the POV tab. This phone's entry follows its live pose.
    @Published private(set) var devices = [PovDevice]()
    private var remoteDevices = [PovDevice]()
    private var selfInHome: simd_float4x4?
    /// A copy of the map for the POV tab; geometry is shared with the AR scene.
    let povScene = SCNScene()
    private let povRoot = SCNNode()
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
    // North is found once per launch with a heading-aligned session. Tracking then
    // drops back to gravity alone: a live compass keeps turning the world under the map.
    private enum NorthSearch { case pending, asking, searching, done }
    private var north = NorthSearch.pending
    private var headingGate = HeadingGate()
    private let locator = GeoLocator()
    /// Pinned to the room where the heading-aligned session had its axes, so north survives the switch to gravity.
    private var northAnchor: UUID?
    private var geoAtNorth: GeoReference?
    /// Home's frame on the Earth. It outlives the AR session, and gives the position back after a rescan.
    private(set) var homeGeo: (frame: String, reference: GeoReference)?
    private var lastGeoUpdate: TimeInterval = 0
    private var geoInAR: GeoReference?
    private var lastCompassUpdate: TimeInterval = 0
    /// Degrees from the top of the screen round to north. Never wrapped, so the marker takes the short way.
    @Published private(set) var northTurn: Double?
    @Published private(set) var geoStatus = ""

    override init() {
        super.init()
        locator.onPermission = { [weak self] in
            // The answer to the permission prompt; a refusal is handled by the next start.
            guard let self, self.north == .asking, self.locator.permission != .undecided else { return }
            self.north = .pending
            if UIApplication.shared.applicationState != .background { self.start() }
        }
        view.scene = SCNScene()
        view.scene.rootNode.addChildNode(root)
        // Soft light from everywhere and a key light from above, for the things in the scene that are lit.
        let ambient = SCNLight()
        ambient.type = .ambient; ambient.intensity = 450
        let key = SCNLight()
        key.type = .directional; key.intensity = 800
        for (light, tilt) in [(ambient, SCNVector3Zero), (key, SCNVector3(-1.0, 0.5, 0))] {
            let holder = SCNNode()
            holder.light = light; holder.eulerAngles = tilt
            view.scene.rootNode.addChildNode(holder)
        }
        povScene.rootNode.addChildNode(povRoot)
        view.session.delegate = self
        Self.session = view.session
        // The robot model is physically shaded; without a light it renders black.
        view.autoenablesDefaultLighting = true
        view.session.delegateQueue = .main
        root.isHidden = true
        volumes.attach(view, root: root) { [weak self] in
            guard let self, self.aligned, !self.snoozed,
                  let frame = self.view.session.currentFrame,
                  case .normal = frame.camera.trackingState else { return false }
            return true
        }
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
        if north == .pending {
            switch locator.permission {
            case .granted:
                north = .searching
                headingGate = HeadingGate()
                locator.start(heading: true)
                // Turning on the compass turns the world, so nothing aligned before it holds.
                if aligned { resetAlignment() }
            case .refused:
                north = .done
                geoStatus = "Location is off for Annotator, so the map has no GPS position."
            case .undecided:
                north = .asking
                locator.requestPermission()
            }
        }
        let config = ARWorldTrackingConfiguration()
        config.worldAlignment = north == .searching ? .gravityAndHeading : .gravity
        // Surfaces are placed on detected floors, tables, walls and screens.
        config.planeDetection = [.horizontal, .vertical]
        if ARWorldTrackingConfiguration.supportsFrameSemantics(.sceneDepth) {
            config.frameSemantics.insert(.sceneDepth)
        } else { depthStatus = "This phone does not provide LiDAR scene depth" }
        // People in front of the map hide it, using their measured depth.
        if ARWorldTrackingConfiguration.supportsFrameSemantics(.personSegmentationWithDepth) {
            config.frameSemantics.insert(.personSegmentationWithDepth)
        }
        view.session.run(config)
    }

    func pause() {
        view.session.pause()
        invalidate()
        worldLost()
    }

    /// Take north from the heading-aligned session once the compass has settled, then switch it off.
    private func searchNorth(_ frame: ARFrame) {
        var tracking = false
        if case .normal = frame.camera.trackingState { tracking = true }
        let fix = locator.fix
        switch headingGate.step(at: frame.timestamp, tracking: tracking, headingError: locator.headingError, hasFix: fix != nil) {
        case .waiting: return
        case .established:
            guard let fix else { return }
            // The session's axes are east, up and south at this moment. Pin them to the room.
            let anchor = ARAnchor(name: "north", transform: matrix_identity_float4x4)
            view.session.add(anchor: anchor)
            northAnchor = anchor.identifier
            let camera = frame.camera.transform.columns.3
            geoAtNorth = GeoReference(fix: fix, at: SIMD3(camera.x, camera.y, camera.z), north: SIMD2(0, -1))
            print("[geo] north established, compass ±\(Int(locator.headingError ?? -1))°, fix ±\(Int(fix.accuracy)) m")
        case .gaveUp:
            geoStatus = "No reliable compass here, so the map has no GPS position."
            print("[geo] \(geoStatus)")
        }
        north = .done
        locator.stopHeading()
        start()
    }

    /// The AR world can no longer be trusted, so neither can a north pinned to it.
    private func worldLost() {
        // An interrupted search is simply started again.
        if north == .searching { headingGate = HeadingGate() }
        geoInAR = nil
        northTurn = nil
        guard geoAtNorth != nil else { return }
        geoAtNorth = nil
        northAnchor = nil
        geoStatus = homeGeo == nil ? "GPS position lost with camera tracking. Relaunch to find north again."
                                   : "GPS position is kept with Home's map. Scan a Portal to restore it."
    }

    private func updateCompass(_ frame: ARFrame) {
        guard frame.timestamp - lastCompassUpdate >= 0.1 else { return }
        lastCompassUpdate = frame.timestamp
        guard let geoInAR else {
            if northTurn != nil { northTurn = nil }
            return
        }
        let camera = frame.camera.transform
        let forward = SIMD2(-camera.columns.2.x, -camera.columns.2.z)
        // Lying flat there is no forward; the top edge of the screen stands in, as on the mini map.
        let top = SIMD2(-camera.columns.0.x, -camera.columns.0.z)
        let target = -geoInAR.bearing(of: simd_length(forward) > 0.3 ? forward : top)
        let previous = northTurn ?? target
        northTurn = previous + (target - previous).remainder(dividingBy: 360)
    }

    private func updateGeo(_ frame: ARFrame) {
        guard frame.timestamp - lastGeoUpdate >= 1 else { return }
        lastGeoUpdate = frame.timestamp
        let homeFrame = map?.scenegraph.map.frame.id
        let world: GeoReference
        if var pinned = geoAtNorth, let id = northAnchor,
           let anchor = frame.anchors.first(where: { $0.identifier == id }) {
            // A better fix than the one north was taken with re-places the reference.
            if let fix = locator.fix, fix.accuracy < pinned.origin.accuracy - 1 {
                let camera = anchor.transform.inverse * frame.camera.transform.columns.3
                pinned = GeoReference(fix: fix, at: SIMD3(camera.x, camera.y, camera.z), north: pinned.north)
                geoAtNorth = pinned
            }
            world = pinned.expressed(in: anchor.transform.inverse)
            if aligned, let homeFrame { homeGeo = (homeFrame, world.expressed(in: root.simdTransform)) }
        } else if aligned, let homeGeo, homeGeo.frame == homeFrame {
            world = homeGeo.reference.expressed(in: root.simdTransform.inverse)
        } else { geoInAR = nil; return }
        geoInAR = world
        let camera = frame.camera.transform
        let here = world.coordinate(of: SIMD3(camera.columns.3.x, camera.columns.3.y, camera.columns.3.z))
        let forward = SIMD2(-camera.columns.2.x, -camera.columns.2.z)
        var text = String(format: "GPS %.6f, %.6f ±%.0f m", here.latitude, here.longitude, here.accuracy)
        if simd_length(forward) > 0.3 { text += String(format: " · facing %.0f°", world.bearing(of: forward)) }
        if let homeGeo, homeGeo.frame == homeFrame {
            let origin = homeGeo.reference.origin
            text += String(format: "\nHome origin %.6f, %.6f · its −Z bears %.0f°", origin.latitude, origin.longitude,
                           homeGeo.reference.bearing(of: SIMD2(0, -1)))
        }
        geoStatus = text
    }

    func load(_ map: PortalMap) {
        // Only changes to the calibration reference reset alignment.
        guard self.map != map else { return }
        // A Portal added to the scene keeps the current alignment; only a changed
        // frame or a moved or removed Portal invalidates it.
        let old = self.map?.scenegraph
        let referenceChanged = old == nil || old?.map != map.scenegraph.map
            || (old?.anchors.contains { map.scenegraph.anchors[$0.key] != $0.value } ?? true)
        if let old, !referenceChanged {
            let added = map.scenegraph.anchors.keys.filter { old.anchors[$0] == nil }.sorted()
            if !added.isEmpty {
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                showToast("Portal \(added.joined(separator: ", ")) added to the scene", good: true)
            }
        }
        self.map = map
        miniMap = MiniMapData(map)
        trackedDevices = trails.record(map.render?.notes ?? [], frame: map.scenegraph.map.frame.id,
                                       ownFrame: "phone-camera:" + poseSession)
        // Everything is redrawn except the point still being carried: taking it out and putting it
        // back would restart its dust trail every time Home sends an update.
        root.childNodes.filter { $0 !== floating }.forEach { $0.removeFromParentNode() }
        for portal in map.scenegraph.anchors.values.sorted(by: { $0.anchor_id < $1.anchor_id }) {
            let node = SCNNode()
            node.simdTransform = portal.pose_in_map.matrix
            let size = CGFloat(portal.side_length_m)
            // An outline preserves the camera view of the printed marker.
            for (w, h, px, py) in [(size, 0.004, 0.0, size/2), (size, 0.004, 0.0, -size/2),
                                   (0.004, size, size/2, 0.0), (0.004, size, -size/2, 0.0)] {
                let line = SCNNode(geometry: SCNPlane(width: w, height: h))
                line.geometry?.firstMaterial?.diffuse.contents = Theme.uiAccent
                line.geometry?.firstMaterial?.lightingModel = .constant
                line.geometry?.firstMaterial?.isDoubleSided = true
                line.position = SCNVector3(Float(px), Float(py), 0.002)
                node.addChildNode(line)
            }
            let text = SCNText(string: portal.anchor_id, extrusionDepth: 0)
            text.font = Theme.uiFont(1, .semibold)
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
        trackedDevices.filter { $0.kind == .robot }.forEach(addTrail)
        for volume in map.render?.volumes ?? [] where volume.valid {
            root.addChildNode(volume.node())
        }
        volumes.redraw()
        povRoot.childNodes.forEach { $0.removeFromParentNode() }
        root.childNodes.forEach { povRoot.addChildNode($0.clone()) }
        applyVolumeVisibility()
        remoteDevices = Self.remoteDevices(map, ownSession: poseSession)
        refreshDevices()
        if referenceChanged { resetAlignment() }
        else if aligned { status = "Aligned · \(map.scenegraph.anchors.count) Portals · \(map.render?.voxels.count ?? 0) voxels" }
    }

    private static func remoteDevices(_ map: PortalMap, ownSession: String) -> [PovDevice] {
        var found = [PovDevice]()
        for note in map.render?.notes ?? [] {
            guard let phone = note.phone_pose, phone.pose.translation.count == 3, phone.pose.rotation_wxyz.count == 4 else { continue }
            // Home names each pose's source frame after the sending AR session,
            // which identifies this phone's own entry exactly.
            found.append(PovDevice(id: "phone:" + phone.peer_id, name: phone.name ?? note.text, symbol: "iphone",
                                   asleep: phone.state == "asleep", transform: PovDevice.upright(phone.pose.matrix),
                                   isSelf: phone.pose.from_frame_id == "phone-camera:" + ownSession))
        }
        var located = Set(found.map(\.id))
        // Robots that Home labels in the scene, each with its base pose.
        for note in map.render?.notes ?? [] {
            guard let robot = note.robot_pose, robot.pose.translation.count == 3, robot.pose.rotation_wxyz.count == 4,
                  robot.pose.translation.allSatisfy(\.isFinite), robot.pose.rotation_wxyz.allSatisfy(\.isFinite) else { continue }
            let base = robot.pose.matrix
            // A camera looks along its -Z with +Y up; the base drives along +X with +Z up.
            var eye = base * simd_float4x4(SIMD4(0, -1, 0, 0), SIMD4(0, 0, 1, 0), SIMD4(-1, 0, 0, 0), SIMD4(0, 0, 0, 1))
            eye.columns.3.y += 0.35
            found.append(PovDevice(id: "robot:" + robot.peer_id, name: robot.name ?? note.text, symbol: "car.side",
                                   asleep: false, transform: eye, chassis: base, chassisZUp: true))
            located.insert("phone:" + robot.peer_id)   // already placed; not also an unplaced peer
        }
        func robot(_ sample: RenderScene.RobotPose?, id: String, name: String) {
            guard let pose = sample?.pose, pose.translation.count == 3, pose.rotation_xyzw.count == 4,
                  sample?.frame == map.scenegraph.map.frame.id else { return }
            // Raise the eye above the chassis so the view is not inside the robot.
            var eye = pose.matrix
            eye.columns.3.y += 0.35
            found.append(PovDevice(id: "robot:" + id, name: name, symbol: "car.side", asleep: false, transform: eye,
                                   chassis: pose.matrix))
        }
        if let runners = map.render?.local_mapping?.runners, !runners.isEmpty {
            for (id, runner) in runners.sorted(by: { $0.key < $1.key }) {
                if runner.robot_pose != nil {
                    robot(runner.robot_pose, id: id, name: "Robot " + String(id.suffix(4)))
                } else if !located.contains("phone:" + id) {
                    // Known to Home but not placed in this scene, so there is no view to show.
                    // Home describes phone contributors as "Phone LiDAR …"; anything else
                    // contributing to a scene is a robot. Only robots are drivable.
                    let isPhone = runner.detail?.hasPrefix("Phone") == true
                    found.append(PovDevice(id: (isPhone ? "peerphone:" : "peer:") + id,
                                           name: (isPhone ? "Phone " : "Robot ") + String(id.suffix(4)),
                                           symbol: isPhone ? "iphone" : "car.side",
                                           asleep: false, transform: matrix_identity_float4x4, located: false,
                                           detail: "no position" + (runner.status.map { " · " + $0 } ?? "")))
                }
            }
        } else { robot(map.render?.local_mapping?.robot_pose, id: "runner", name: "Robot") }
        return found.sorted { ($0.located ? 0 : 1, $0.name) < ($1.located ? 0 : 1, $1.name) }
    }

    private func refreshDevices() {
        var list = [PovDevice]()
        if let selfInHome {
            list.append(PovDevice(id: "self", name: "This phone", symbol: "iphone.gen3", asleep: false,
                                  transform: PovDevice.upright(selfInHome)))
        }
        // Home lists this phone too. Its copy lags the live pose, so never show both.
        list += remoteDevices.filter { !$0.isSelf }
        if list != devices { devices = list }
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
                var inner = [SCNVector3]()
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
                        let q = center + corner * scene.voxel_size * 0.45
                        inner.append(SCNVector3(q.x, q.y, q.z))
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
                // The voxels are see-through, and SceneKit does not sort triangles inside one mesh,
                // so drawn plainly they show through each other in whatever order Home listed them.
                // A slightly smaller copy of every voxel goes down first as depth with no colour.
                // Colour then lands on the nearest faces only. The copy sits just inside the coloured
                // faces, so the two never share a depth and cannot flicker against each other.
                let depth = SCNMaterial()
                depth.lightingModel = .constant
                depth.colorBufferWriteMask = []
                let shell = SCNGeometry(sources: [SCNGeometrySource(vertices: inner)],
                    elements: [SCNGeometryElement(indices: indices, primitiveType: .triangles)])
                shell.materials = [depth]
                let first = SCNNode(geometry: shell)
                first.renderingOrder = -2
                root.addChildNode(first)
                let material = SCNMaterial()
                material.lightingModel = .constant
                material.writesToDepthBuffer = false
                geometry.materials = [material]
                let second = SCNNode(geometry: geometry)
                second.renderingOrder = -1
                root.addChildNode(second)
            }
        }
        for object in scene.objects { addLabel(object.label, at: object.position, color: Theme.uiAccent) }
        // Home's device labels stay plain; a person's note is a speech bubble.
        for note in scene.notes {
            if note.phone_pose == nil && note.robot_pose == nil { addBubble(note.text, at: note.position, photo: photos[note.id]) }
            else { addLabel(note.text, at: note.position, color: .white) }
        }
        // Once Home sends a point back, its copy takes over from the one kept here.
        unsynced.removeAll { kept in scene.notes.contains { $0.id == kept.id } }
        for kept in unsynced where kept.frame == scene.frame { addBubble(kept.text, at: kept.position, photo: photos[kept.id]) }
        for note in scene.notes {
            guard let pose = note.robot_pose?.pose, pose.translation.count == 3, pose.rotation_wxyz.count == 4 else { continue }
            root.addChildNode(robotBody(at: pose.matrix))
        }
        if let runners = scene.local_mapping?.runners, !runners.isEmpty {
            for (id, runner) in runners { addRobot(runner.robot_pose, name: String(id.suffix(8))) }
        } else { addRobot(scene.local_mapping?.robot_pose, name: "Runner") }
    }

    /// A note as a speech bubble whose tail points at the spot it was left on.
    private func addBubble(_ title: String, at position: [Float], photo: UIImage? = nil) {
        guard position.count == 3, position.allSatisfy(\.isFinite) else { return }
        let bubble = Self.bubble(title, photo: photo)
        bubble.position = SCNVector3(position[0], position[1], position[2])
        root.addChildNode(bubble)
    }
    /// The AR session, for things drawn in the scene that need to know where the phone is.
    private static weak var session: ARSession?
    /// A speech bubble of one fixed size with its origin at the tip of its tail.
    /// The words shrink and wrap to fit it. With no words yet it asks for a name.
    /// A photo, if the note has one, sits above it as a rounded square.
    private static func bubble(_ title: String, photo: UIImage? = nil) -> SCNNode {
        // Fixed colours: a bubble in the camera view does not follow light or dark.
        let light = UITraitCollection(userInterfaceStyle: .light)
        // Drawn in centimetres and scaled down: SceneKit cuts curves this small into too few pieces in metres.
        let width: CGFloat = 5.5, height: CGFloat = 2.4, pad: CGFloat = 0.4, tail: CGFloat = 0.7, thick: CGFloat = 0.3
        let prompt = title.isEmpty
        // One outline, tail included: round corners, and a point at the middle of the bottom edge.
        let r: CGFloat = 1.0
        let outline = UIBezierPath()
        outline.move(to: CGPoint(x: r, y: 0))
        outline.addLine(to: CGPoint(x: width / 2 - 0.45, y: 0))
        outline.addLine(to: CGPoint(x: width / 2, y: -tail))
        outline.addLine(to: CGPoint(x: width / 2 + 0.45, y: 0))
        for (cx, cy, start) in [(width - r, r, -CGFloat.pi / 2), (width - r, height - r, 0), (r, height - r, .pi / 2), (r, r, .pi)] {
            outline.addArc(withCenter: CGPoint(x: cx, y: cy), radius: r, startAngle: start, endAngle: start + .pi / 2, clockwise: true)
        }
        outline.close()
        outline.flatness = 0.005
        // A little thickness with rounded edges, and a lit material, so it reads as a solid thing in the room.
        let shape = SCNShape(path: outline, extrusionDepth: thick)
        shape.chamferRadius = thick * 0.45
        // A quarter circle in many steps: the default bevel is a single flat cut.
        let bevel = UIBezierPath()
        bevel.move(to: CGPoint(x: 0, y: 1))
        bevel.addQuadCurve(to: CGPoint(x: 1, y: 0), controlPoint: CGPoint(x: 1, y: 1))
        bevel.flatness = 0.01
        shape.chamferProfile = bevel
        shape.firstMaterial?.diffuse.contents = Theme.uiPaper.resolvedColor(with: light)
        // Lit for its rounded edges and shine, with a little glow of its own so it never goes dull in a dim room.
        shape.firstMaterial?.lightingModel = .blinn
        shape.firstMaterial?.specular.contents = UIColor(white: 1, alpha: 0.6)
        shape.firstMaterial?.shininess = 0.4
        shape.firstMaterial?.emission.contents = Theme.uiPaper.resolvedColor(with: light)
        shape.firstMaterial?.emission.intensity = 0.15
        // The drawing, in centimetres, moved so the tip of the tail is at the origin and scaled to metres.
        // The node that is returned carries no scale or pivot of its own: it only turns to face the camera.
        let drawing = SCNNode()
        drawing.simdScale = SIMD3(repeating: 0.01)
        drawing.simdPosition = SIMD3(Float(-width / 2), Float(tail), 0) * 0.01
        drawing.addChildNode(SCNNode(geometry: shape))
        // The words are painted as a picture, at the largest size that fits the frame on however many
        // lines that takes. A picture also shows emoji, which SceneKit's own text leaves blank.
        let frame = CGSize(width: (width - 2 * pad) * 120, height: (height - 2 * pad) * 120)
        let centred = NSMutableParagraphStyle()
        centred.alignment = .center
        centred.lineBreakMode = .byWordWrapping
        func words(_ size: CGFloat) -> NSAttributedString {
            NSAttributedString(string: prompt ? "Note name" : title, attributes: [
                .font: Theme.uiFont(size, .semibold), .paragraphStyle: centred,
                .foregroundColor: (prompt ? Theme.uiDim : Theme.uiInk).resolvedColor(with: light)])
        }
        func extent(_ size: CGFloat) -> CGSize {
            words(size).boundingRect(with: CGSize(width: frame.width, height: .greatestFiniteMagnitude),
                                     options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil).size
        }
        // One short word does not fill the bubble: at most 45% of its height.
        var size = frame.height * 0.45
        while size > 6, extent(size).height > frame.height || extent(size).width > frame.width { size *= 0.92 }
        let tall = extent(size).height
        let picture = UIGraphicsImageRenderer(size: frame).image { _ in
            words(size).draw(with: CGRect(x: 0, y: (frame.height - tall) / 2, width: frame.width, height: tall),
                             options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
        }
        let face = SCNPlane(width: width - 2 * pad, height: height - 2 * pad)
        face.firstMaterial?.diffuse.contents = picture
        face.firstMaterial?.lightingModel = .constant
        face.firstMaterial?.writesToDepthBuffer = false
        let label = SCNNode(geometry: face)
        // Centred on the bubble's face, a hair in front of it.
        label.simdPosition = SIMD3(Float(width) / 2, Float(height) / 2, Float(thick) / 2 + 0.05)
        label.renderingOrder = 1
        drawing.addChildNode(label)
        if let photo {
            let side: CGFloat = 3.4
            let print = SCNPlane(width: side, height: side)
            print.cornerRadius = 0.6
            print.firstMaterial?.diffuse.contents = photo
            print.firstMaterial?.lightingModel = .constant
            print.firstMaterial?.isDoubleSided = true
            let picture = SCNNode(geometry: print)
            picture.simdPosition = SIMD3(Float(width) / 2, Float(height + 0.35 + side / 2), 0)
            drawing.addChildNode(picture)
        }
        // Life-size up close, a note is too small to read from across a room. It grows with distance
        // from 40 cm out, so it holds its size on screen, up to six times as big.
        let sizer = SCNNode()
        sizer.addChildNode(drawing)
        sizer.constraints = [SCNTransformConstraint(inWorldSpace: false) { node, transform in
            guard let eye = session?.currentFrame?.camera.transform.columns.3 else { return transform }
            let far = simd_distance(node.presentation.simdWorldPosition, SIMD3(eye.x, eye.y, eye.z))
            let grow = min(6, max(1, far / 0.4))
            return SCNMatrix4MakeScale(grow, grow, grow)
        }]
        let bubble = SCNNode()
        bubble.addChildNode(sizer)
        bubble.constraints = [SCNBillboardConstraint()]
        return bubble
    }

    private func addLabel(_ title: String, at position: [Float], color: UIColor) {
        guard position.count == 3, position.allSatisfy(\.isFinite) else { return }
        let text = SCNText(string: title, extrusionDepth: 0)
        text.font = Theme.uiFont(1, .semibold)
        text.flatness = 0.2
        text.firstMaterial?.diffuse.contents = color
        text.firstMaterial?.lightingModel = .constant
        let label = SCNNode(geometry: text)
        label.scale = SCNVector3(0.05, 0.05, 0.05)
        label.position = SCNVector3(position[0], position[1], position[2])
        label.constraints = [SCNBillboardConstraint()]
        root.addChildNode(label)
    }

    /// A robot's path as a ribbon just above the floor. A robot Home no longer
    /// reports also keeps a faint body and label where it was last seen.
    /// The robot's 3D model standing on its base pose. Until the model has loaded,
    /// a box the size of the chassis stands in for it.
    private func robotBody(at base: simd_float4x4) -> SCNNode {
        let holder = SCNNode()
        holder.simdTransform = base
        if let template = RobotMapLayer.template {
            // The model and the base frame are both Z up, in metres.
            holder.addChildNode(RobotGlow.outlined(template.clone()))
        } else {
            RobotMapLayer.whenLoaded {}
            // Base frame is Z up, +X forward: 0.5 m long, 0.35 m wide, 0.2 m tall, sitting on its origin.
            let box = SCNNode(geometry: SCNBox(width: 0.5, height: 0.35, length: 0.2, chamferRadius: 0.03))
            box.geometry?.firstMaterial?.diffuse.contents = UIColor.white
            box.geometry?.firstMaterial?.lightingModel = .constant
            box.simdPosition = SIMD3(0, 0, 0.1)
            holder.addChildNode(RobotGlow.outlined(box))
        }
        return holder
    }

    private func addTrail(_ robot: TrackedDevice) {
        if robot.state == .lost {
            let body = robotBody(at: robot.pose)
            body.opacity = 0.3
            root.addChildNode(body)
            let p = robot.position
            addLabel(robot.name + " zzz", at: [p.x, p.y, p.z], color: UIColor.white.withAlphaComponent(0.3))
        }
        let path = robot.trail + [robot.position]
        var vertices = [SCNVector3](), colors = [Float](), indices = [UInt32]()
        for i in 1..<path.count {
            let a = path[i - 1], b = path[i]
            let along = SIMD3(b.x - a.x, 0, b.z - a.z)
            guard simd_length(along) > 1e-4 else { continue }
            let side = simd_normalize(simd_cross(SIMD3(0, 1, 0), along)) * 0.03, lift = SIMD3<Float>(0, 0.03, 0)
            let base = UInt32(vertices.count)
            // Oldest is pale blue, newest is yellow.
            let older = DeviceTrails.color(age: Float(i - 1) / Float(path.count - 1))
            let newer = DeviceTrails.color(age: Float(i) / Float(path.count - 1))
            let low = a + lift, high = b + lift
            let corners: [(SIMD3<Float>, SIMD3<Float>)] = [(low - side, older), (low + side, older), (high + side, newer), (high - side, newer)]
            for (p, c) in corners {
                vertices.append(SCNVector3(p.x, p.y, p.z))
                colors += [c.x, c.y, c.z, 1]
            }
            indices += [base, base + 1, base + 2, base, base + 2, base + 3]
        }
        guard !indices.isEmpty else { return }
        let colorSource = SCNGeometrySource(data: colors.withUnsafeBytes { Data($0) }, semantic: .color,
            vectorCount: vertices.count, usesFloatComponents: true, componentsPerVector: 4,
            bytesPerComponent: 4, dataOffset: 0, dataStride: 16)
        let ribbon = SCNGeometry(sources: [SCNGeometrySource(vertices: vertices), colorSource],
                                 elements: [SCNGeometryElement(indices: indices, primitiveType: .triangles)])
        let material = SCNMaterial()
        material.lightingModel = .constant
        material.isDoubleSided = true
        ribbon.materials = [material]
        let trail = SCNNode(geometry: ribbon)
        trail.name = "robot-trail"
        trail.isHidden = !showTrails
        root.addChildNode(trail)
    }

    private func addRobot(_ sample: RenderScene.RobotPose?, name: String) {
        guard let pose = sample?.pose, pose.translation.count == 3, pose.rotation_xyzw.count == 4,
              sample?.frame == map?.scenegraph.map.frame.id else { return }
        let robot = SCNNode(geometry: SCNBox(width: 0.35, height: 0.2, length: 0.5, chamferRadius: 0.03))
        robot.geometry?.firstMaterial?.diffuse.contents = UIColor.white
        robot.geometry?.firstMaterial?.lightingModel = .constant
        robot.simdTransform = pose.matrix
        root.addChildNode(robot)
        var position = pose.translation; position[1] += 0.3
        addLabel(name, at: position, color: .white)
    }

    func clearPlacement() {
        placement = nil
        floating = nil; floatingText = ""; floatingPhoto = nil
        root.childNode(withName: "annotation-preview", recursively: false)?.removeFromParentNode()
    }

    @objc private func doubleTap(_ gesture: UITapGestureRecognizer) {
        guard volumes.draft == nil else { return }
        if aligned, let map {
            let hits = view.hitTest(gesture.location(in: view), options: [:])
            if let name = hits.first(where: { $0.node.name?.hasPrefix("semantic-volume:") == true })?.node.name,
               let volume = map.render?.volumes?.first(where: { "semantic-volume:" + $0.id == name }) {
                volumes.begin(frame: map.scenegraph.map.frame.id, existing: volume)
                return
            }
        }
        placeNote(at: gesture.location(in: view))
    }

    private func placeNote(at point: CGPoint) {
        guard aligned, let map, let frame = view.session.currentFrame,
              case .normal = frame.camera.trackingState, placement == nil else {
            status = "Scan a Portal before placing a note."; return
        }
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
        marker.geometry?.firstMaterial?.diffuse.contents = Theme.uiAccent
        marker.geometry?.firstMaterial?.lightingModel = .constant
        root.addChildNode(marker)
        placement = NotePlacement(id: UUID().uuidString, frame: map.scenegraph.map.frame.id,
            origin: [origin.x, origin.y, origin.z], direction: [direction.x, direction.y, direction.z])
    }

    func clear() {
        clearPlacement()
        map = nil
        miniMap = MiniMapData()
        trails = DeviceTrails(); trackedDevices = []
        povRoot.childNodes.forEach { $0.removeFromParentNode() }
        remoteDevices = []; refreshDevices()
        root.childNodes.forEach { $0.removeFromParentNode() }
        invalidate()
        status = "Connect to Home to receive its scene"
    }

    private func invalidate() {
        volumes.invalidate()
        onPose?(nil)
        onDepth?(nil)
        depthAlignment = nil
        alignedThrough = [:]
        depthStatus = shareDepth ? "Scan a Portal to share depth" : "Depth sharing off"
        generation += 1; aligned = false; cooldown.reset(); root.isHidden = true
        alignment.reset()
        miniPhone = nil
        if selfInHome != nil { selfInHome = nil; refreshDevices() }
    }

    func realign() {
        resetAlignment()
    }

    /// Drop the current alignment. Nothing is mapped again until a Portal is found.
    private func resetAlignment() {
        invalidate()
        status = "\(map?.scenegraph.anchors.count ?? 0) Portals loaded. Scan a known Portal."
    }

    private var toast: UILabel?
    private func showToast(_ text: String, good: Bool) {
        toast?.removeFromSuperview()
        let label = UILabel()
        label.text = text; label.numberOfLines = 0; label.textAlignment = .center
        label.font = Theme.uiFont(14, .semibold)
        label.textColor = good ? Theme.uiInk : UIColor(white: 0.1, alpha: 1)
        label.backgroundColor = (good ? Theme.uiPaper : Theme.uiAccent).withAlphaComponent(0.94)
        label.layer.borderWidth = 1
        label.layer.borderColor = Theme.uiInk.resolvedColor(with: view.traitCollection).cgColor
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
        layer.path = path.cgPath; layer.fillColor = UIColor.clear.cgColor; layer.lineWidth = 3
        layer.strokeColor = (good ? UIColor.white : Theme.uiAccent).cgColor
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
        guard volumes.draft == nil else { return }
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
            let imageWidth = Float(CVPixelBufferGetWidth(image)), imageHeight = Float(CVPixelBufferGetHeight(image))
            let codes = (request.results ?? []).map { code -> (String?, [CGPoint], simd_float4x4?, [SIMD2<Float>]) in
                let raw = [code.topLeft, code.topRight, code.bottomRight, code.bottomLeft]
                let corners = raw.map { p -> CGPoint in
                    // Vision: origin bottom-left. ARKit image space: origin top-left.
                    let shown = CGPoint(x: p.x, y: 1 - p.y).applying(display)
                    return CGPoint(x: shown.x * size.width, y: shown.y * size.height)
                }
                // Pose at the assumed size, in case this is a Portal the scene lacks.
                let pixels = raw.map { SIMD2(Float($0.x) * imageWidth, (1 - Float($0.y)) * imageHeight) }
                let pose = PortalPose.estimate(corners: pixels, side: PortalReport.assumedSide, intrinsics: frame.camera.intrinsics)
                return (code.payloadStringValue, corners, pose.map { frame.camera.transform * $0 }, pixels)
            }
            DispatchQueue.main.async {
                guard let (payload, corners, portalInAR, pixels) = codes.first(where: { $0.0.flatMap(PortalMap.portalCode) != nil }) ?? codes.first else {
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
                    // A long-press on an unknown Portal asks Home to add it right away.
                    guard self.aligned, self.depthAlignment != nil else {
                        self.showOutline(corners, good: false)
                        UINotificationFeedbackGenerator().notificationOccurred(.warning)
                        self.showToast("Portal \(code) is new. Scan a Portal this scene already knows first, then long-press this one again.", good: false)
                        return
                    }
                    guard let portalInAR else {
                        self.showOutline(corners, good: false)
                        self.showToast("Portal \(code) is new, but I can't measure it from here. Face it more directly.", good: false)
                        return
                    }
                    self.showOutline(corners, good: true)
                    self.offerNewPortal(code, portalInAR: portalInAR, at: frame.timestamp, confirmed: true)
                    return
                }
                self.showOutline(corners, good: true)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                // A target the scene knows: trust Home's record of it, or correct that record.
                let sheet = UIAlertController(title: "Target \(code)", message: nil, preferredStyle: .actionSheet)
                sheet.addAction(UIAlertAction(title: "Align to this target", style: .default) { _ in
                    self.showToast("Portal \(code) found. Aligning…", good: true)
                    // Scan again on the very next frame, ignoring the cooldown.
                    self.cooldown.reset(); self.lastAttempt = 0
                })
                sheet.addAction(UIAlertAction(title: "Edit target: it has moved to here", style: .destructive) { _ in
                    self.moveTarget(code, pixels: pixels, frame: frame)
                })
                sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
                self.view.window?.rootViewController?.present(sheet, animated: true)
            }
        }
    }

    /// Someone moved a printed target. Tell Home where it is now, measured from this
    /// phone's alignment through a different target, which must not have moved.
    private func moveTarget(_ code: String, pixels: [SIMD2<Float>], frame: ARFrame) {
        guard let map, let anchor = map.scenegraph.anchors[code] else { return }
        let home = map.scenegraph.map.frame.id
        guard !home.hasSuffix(":portal-" + code) else {
            showToast("Target \(code) is this scene's origin, so it stays put. Move the other targets to match it.", good: false)
            return
        }
        guard let (via, reference) = alignedThrough.filter({ $0.key != code }).max(by: { $0.value.time < $1.value.time }),
              let known = map.scenegraph.anchors[via] else {
            showToast("Scan a target that has not moved first, then long-press \(code) again.", good: false)
            return
        }
        guard let seen = PortalPose.estimate(corners: pixels, side: anchor.side_length_m, intrinsics: frame.camera.intrinsics) else {
            showToast("Can't measure target \(code) from here. Face it more directly.", good: false)
            return
        }
        let portalInAR = frame.camera.transform * seen
        let session = "phone-ar:\(poseSession)"
        let alignment = DepthAlignment(timestamp_ns: UInt64(reference.time * 1_000_000_000), portal_id: via,
            portal_to_session: .transform(reference.homeInAR * known.pose_in_map.matrix, from: known.pose_in_map.from_frame_id, to: session),
            session_to_home: .transform(reference.homeInAR.inverse, from: session, to: home))
        let report = PortalReport(session: poseSession, timestamp_ns: UInt64(max(frame.timestamp, reference.time) * 1_000_000_000),
            code: code, side_length_m: anchor.side_length_m,
            portal_to_session: .transform(portalInAR, from: anchor.pose_in_map.from_frame_id, to: session),
            alignment: alignment, replace: true)
        let now = reference.homeInAR.inverse * portalInAR
        let distance = simd_distance(now.columns.3, anchor.pose_in_map.matrix.columns.3)
        if onNewPortal?(report) == true {
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            showToast(String(format: "Moving target \(code) %.2f m, measured from \(via)…", distance), good: true)
        } else {
            showToast("Home is not connected, so target \(code) was not moved.", good: false)
        }
    }

    /// Report an unknown Portal once two measurements a moment apart agree, so
    /// one noisy read cannot place it. Needs an alignment from a known Portal.
    func showPortalResult(_ text: String) {
        let good = text.contains("added to Home") || text.contains("moved in Home")
        UINotificationFeedbackGenerator().notificationOccurred(good ? .success : .error)
        // An older Home has no such operation; say so plainly.
        showToast(good || !text.contains("did not answer") ? text : text + ". This Home may not support this yet.", good: good)
    }

    private func offerNewPortal(_ code: String, portalInAR: simd_float4x4, at time: TimeInterval, confirmed: Bool = false) {
        guard aligned, let alignment = depthAlignment, map?.scenegraph.anchors[code] == nil else {
            if aligned, depthAlignment == nil { status = "Scan a known Portal first, then Portal \(code) can be added." }
            return
        }
        if !confirmed, let last = portalReported[code], time - last < 6 { return }
        let position = SIMD3(portalInAR.columns.3.x, portalInAR.columns.3.y, portalInAR.columns.3.z)
        // An explicit long-press sends at once; passive sightings need two agreeing reads.
        if confirmed { portalCandidates[code] = (position, time) }
        guard let previous = portalCandidates[code], confirmed || (time - previous.time < 3 &&
              simd_distance(previous.position, position) < 0.03) else {
            portalCandidates[code] = (position, time)
            status = "New Portal \(code) seen. Hold still to add it."
            return
        }
        portalCandidates[code] = nil
        portalReported[code] = time
        let report = PortalReport(session: poseSession, timestamp_ns: UInt64(time * 1_000_000_000), code: code,
            side_length_m: PortalReport.assumedSide,
            portal_to_session: .transform(portalInAR, from: "portal:\(code):local", to: "phone-ar:\(poseSession)"),
            alignment: alignment)
        if onNewPortal?(report) == true {
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            showToast("New Portal \(code) found. Asking Home to add it at 15 cm…", good: true)
        } else {
            showToast("New Portal \(code) found, but Home is not connected.", good: false)
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
        worldLost()
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
        status = map == nil ? "Connect to Home to receive its scene" : "Awake. Scan a Portal to start mapping again."
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
        if north == .searching {
            searchNorth(frame)
            // This frame, and any already on their way, are still in the compass's axes.
            if north != .searching { lastAttempt = frame.timestamp + 0.5; return }
        }
        if map == nil { scanBeforeMap(frame); return }
        guard case .normal = frame.camera.trackingState else {
            volumes.invalidate()
            if map != nil {
                let message: String
                switch frame.camera.trackingState {
                case .limited(.initializing): message = "Camera tracking is starting. Move the phone slowly."
                case .limited(.insufficientFeatures): message = "Camera needs more detail. Include the Portal's surroundings."
                case .limited(.excessiveMotion): message = "Move the phone more slowly to scan."
                case .limited(.relocalizing): message = "Recovering camera tracking. Look around slowly."
                default: message = "Camera tracking unavailable. Tap Realign to try again."
                }
                // While aligned, a dip in tracking only pauses mapping.
                let shown = aligned ? "Tracking limited · mapping paused until it recovers" : message
                if status != shown { status = shown }
            }
            return
        }
        // The compass is still turning the world; an alignment made now would not hold.
        if north == .searching {
            let message = "Finding north. Hold the phone up and move it slowly."
            if status != message { status = message }
            return
        }
        updateGeo(frame)
        updateCompass(frame)
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
                miniPhone = MiniMapPhone(position: SIMD3(cameraInHome.columns.3.x, cameraInHome.columns.3.y, cameraInHome.columns.3.z),
                                         heading: heading.x.isFinite ? heading : SIMD2(0, -1))
                selfInHome = cameraInHome
                refreshDevices()
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
            var found: (String, simd_float4x4, simd_float4x4, Float)?
            var unknown: (String, simd_float4x4)?
            for code in request.results ?? [] {
                guard let payload = code.payloadStringValue else {
                    feedback = "QR visible but unreadable. Move closer and avoid glare."
                    continue
                }
                guard let portal = map.matching(payload) else {
                    if let newCode = PortalMap.portalCode(payload) {
                        feedback = "Portal \(newCode) is not in this scene yet."
                        // Measure it at the assumed size so Home can be asked to add it.
                        let corners = [code.topLeft, code.topRight, code.bottomRight, code.bottomLeft]
                            .map { SIMD2(Float($0.x) * width, (1 - Float($0.y)) * height) }
                        if unknown == nil, let pose = PortalPose.estimate(corners: corners, side: PortalReport.assumedSide,
                                                                          intrinsics: frame.camera.intrinsics) {
                            unknown = (newCode, frame.camera.transform * pose)
                        }
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
                var edge: Float = 0
                for i in 0..<4 { edge += simd_length(corners[i] - corners[(i + 1) % 4]) }
                found = (portal.anchor_id, portalInAR * portal.pose_in_map.matrix.inverse, portalInAR, edge / 4)
                break
            }
            let result = found
            let candidate = unknown
            let scanFeedback = feedback
            DispatchQueue.main.async {
                self.busy = false
                guard self.generation == capturedGeneration, self.placement == nil else { return }
                guard case .normal = self.view.session.currentFrame?.camera.trackingState else { return }
                if let (code, portalInAR) = candidate { self.offerNewPortal(code, portalInAR: portalInAR, at: frame.timestamp) }
                guard let (id, measured, portalInAR, edgePixels) = result, let portal = map.scenegraph.anchors[id] else {
                    if !self.aligned { self.status = scanFeedback }
                    return
                }
                // One sighting is one vote. The fused alignment only turns about
                // gravity, so a tilted or hand-held Portal cannot tip the world.
                let stored = portal.pose_in_map.matrix.columns.3
                let outcome = self.alignment.add(measured: measured,
                    portalInAR: SIMD3(portalInAR.columns.3.x, portalInAR.columns.3.y, portalInAR.columns.3.z),
                    portalInHome: SIMD3(stored.x, stored.y, stored.z), edgePixels: edgePixels)
                let transform = self.alignment.transform
                self.root.simdTransform = transform
                self.root.isHidden = false
                self.aligned = true
                // Home checks that the session-to-scene transform follows from the
                // Portal's pose in the session, so report the Portal where the fused
                // alignment places it, not where this single read did.
                let sessionFrame = "phone-ar:\(self.poseSession)"
                self.alignedThrough[id] = (transform, frame.timestamp)
                self.depthAlignment = DepthAlignment(timestamp_ns: UInt64(frame.timestamp * 1_000_000_000), portal_id: id,
                    portal_to_session: .transform(transform * portal.pose_in_map.matrix, from: portal.pose_in_map.from_frame_id, to: sessionFrame),
                    session_to_home: .transform(transform.inverse, from: sessionFrame, to: map.scenegraph.map.frame.id))
                // Start the one-second pause when calibration is applied, not
                // when Vision began processing the captured frame.
                self.cooldown.accept(at: self.view.session.currentFrame?.timestamp ?? frame.timestamp)
                let voxels = self.map?.render?.voxels.count ?? 0
                switch outcome {
                case .locked, .relocked: self.status = "Aligned with \(id) · \(voxels) voxels"
                case .averaged: self.status = "Aligned · refined with \(id) (\(self.alignment.sightings) reads) · \(voxels) voxels"
                case .ignoredTilted: self.status = "Aligned · \(id) looks tilted or hand-held, so it was not used"
                case .ignoredOutlier: self.status = "Aligned · ignored one read of \(id) that disagreed"
                }

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
                self.depthStatus = "LiDAR active · \(sample.points.count) \(sample.colors == nil ? "" : "colored ")voxels per capture"
                self.onDepth?(sample)
            }
        }
    }

    func session(_ session: ARSession, cameraDidChangeTrackingState camera: ARCamera) {
        if snoozed { return }
        switch camera.trackingState {
        case .normal:
            if aligned { status = "Tracking recovered · mapping resumed" }
        case .limited(.excessiveMotion), .limited(.insufficientFeatures), .limited(.initializing):
            // ARKit keeps its world origin through these, so the alignment still
            // holds. Frames are simply not used until tracking is normal again;
            // dropping the alignment here meant mapping only ran with a Portal in view.
            if aligned { status = "Tracking limited · mapping paused until it recovers" }
        default:
            // Relocalizing or unavailable: the world may have moved under us.
            invalidate()
            worldLost()
            status = "Tracking lost. Scan a known Portal again."
        }
    }
    func sessionWasInterrupted(_ session: ARSession) {
        if snoozed { return }
        invalidate(); worldLost(); status = "AR interrupted. Rescan a Portal when the camera returns."
    }
    func sessionInterruptionEnded(_ session: ARSession) { if snoozed { return }; realign(); start() }
    func session(_ session: ARSession, didFailWithError error: Error) {
        invalidate(); worldLost()
        // A heading-aligned session that cannot run must not cost the app its camera.
        if north == .searching {
            north = .done
            locator.stopHeading()
            geoStatus = "Could not find north (\(error.localizedDescription)), so the map has no GPS position."
            start()
            return
        }
        status = error.localizedDescription
    }
}


/// A small card for naming a new point. The camera stays in view behind it.
struct AnnotationEditor: View {
    let controller: ViewerController
    @ObservedObject var peer: HomePeer
    let placement: NotePlacement
    let close: () -> Void
    @State private var text = ""
    @State private var message = ""
    @State private var photo: UIImage?
    @State private var choosing = false
    @State private var chosen: PhotosPickerItem?
    @FocusState private var typing: Bool
    private var name: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Point").themeLabel(13, .medium)
            TextField("Note name", text: $text).focused($typing).submitLabel(.done).onSubmit(save)
                .onChange(of: text) { _, value in controller.namePoint(value.trimmingCharacters(in: .whitespacesAndNewlines)) }
            HStack {
                Button("Cancel", action: close)
                // Only a point carried by the phone shows a photo above it.
                if placement.at != nil {
                    if let photo {
                        Button { self.photo = nil } label: {
                            HStack(spacing: 6) {
                                Image(uiImage: photo).resizable().frame(width: 22, height: 22)
                                    .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                                Image(systemName: "xmark")
                            }
                        }
                        .accessibilityLabel("Remove the photo")
                    } else {
                        Menu {
                            Button { photo = controller.cameraPhoto() } label: { Label("Take photo", systemImage: "camera") }
                            Button { choosing = true } label: { Label("Choose from library", systemImage: "photo.on.rectangle") }
                        } label: {
                            Label("Photo", systemImage: "camera.fill").themeLabel(12, .bold)
                                .padding(.horizontal, 16).padding(.vertical, 10)
                                .foregroundStyle(Theme.ink).background(Theme.well, in: Capsule())
                        }
                    }
                }
                Spacer()
                Button("Save", action: save)
                    .disabled(name.isEmpty || text.count > 120)
                    .buttonStyle(PillButtonStyle(filled: true))
            }
            if !message.isEmpty { Text(message).themeText(10).foregroundStyle(Theme.dim) }
        }
        .textFieldStyle(SoftFieldStyle())
        .buttonStyle(PillButtonStyle())
        .themePanel(padding: 16)
        .onAppear { typing = true }
        .onChange(of: photo) { _, value in controller.setPointPhoto(value) }
        .photosPicker(isPresented: $choosing, selection: $chosen, matching: .images)
        .onChange(of: chosen) { _, item in
            guard let item else { return }
            Task {
                guard let data = try? await item.loadTransferable(type: Data.self), let picked = UIImage(data: data) else { return }
                // Cut square from the middle and made small, like a photo from the camera.
                let fill = 720 / min(picked.size.width, picked.size.height)
                let size = CGSize(width: picked.size.width * fill, height: picked.size.height * fill)
                let format = UIGraphicsImageRendererFormat()
                format.scale = 1
                photo = UIGraphicsImageRenderer(size: CGSize(width: 720, height: 720), format: format).image { _ in
                    picked.draw(in: CGRect(x: (720 - size.width) / 2, y: (720 - size.height) / 2, width: size.width, height: size.height))
                }
                chosen = nil
            }
        }
    }
    private func save() {
        guard !name.isEmpty, text.count <= 120 else { return }
        // A point carried by the phone is fixed where it is at this moment.
        guard let fixed = placement.at == nil ? placement : controller.lockPoint(placement) else {
            message = "Hold still for a moment, then save again."; return
        }
        // Out of reach of Home, the note is kept on the phone and synced when Home is back.
        controller.keepPoint(fixed, text: name)
        peer.saveOrQueue(fixed, text: name)
        close()
    }
}


enum PhoneDepthCapture {
    /// Home's voxel edge. Cells are aligned to Home's frame so a sent voxel
    /// center lands in exactly one Home cell.
    static let voxelSize: Float = 0.1
    static let maxVoxels = 2048

    private static func linear(_ byte: Float) -> Float {
        let c = byte / 255
        return c <= 0.04045 ? c / 12.92 : powf((c + 0.055) / 1.055, 2.4)
    }
    private static func srgbByte(_ linear: Float) -> UInt8 {
        let c = max(0, min(1, linear))
        let encoded = c <= 0.0031308 ? c * 12.92 : 1.055 * powf(c, 1 / 2.4) - 0.055
        return UInt8(max(0, min(255, (encoded * 255).rounded())))
    }

    /// Voxelize one LiDAR capture on the phone. Every high-confidence depth
    /// pixel is unprojected, moved into Home's frame, and binned into 10 cm
    /// cells; each cell's camera color is averaged in linear light. The result
    /// is one colored point per occupied voxel (its center), expressed back in
    /// this capture's camera frame so Home can still validate pose and rays.
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
        let imageWidth = CVPixelBufferGetWidth(frame.capturedImage), imageHeight = CVPixelBufferGetHeight(frame.capturedImage)

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
        /// Linear-light RGB of the camera pixel behind a depth pixel.
        func linearColor(x: Int, y: Int) -> SIMD3<Float>? {
            guard let luma, let chroma else { return nil }
            let px = min(imageWidth - 1, x * imageWidth / width), py = min(imageHeight - 1, y * imageHeight / height)
            var yValue = Float(luma.load(fromByteOffset: py * lumaRow + px, as: UInt8.self))
            let offset = (py / 2) * chromaRow + (px / 2) * 2
            var cb = Float(chroma.load(fromByteOffset: offset, as: UInt8.self)) - 128
            var cr = Float(chroma.load(fromByteOffset: offset + 1, as: UInt8.self)) - 128
            if !fullRange { yValue = (yValue - 16) * 255 / 219; cb *= 255 / 224; cr *= 255 / 224 }
            // BT.601 YCbCr to gamma-encoded sRGB, then to linear for averaging.
            let r = yValue + 1.402 * cr, g = yValue - 0.344136 * cb - 0.714136 * cr, b = yValue + 1.772 * cb
            return SIMD3(linear(max(0, min(255, r))), linear(max(0, min(255, g))), linear(max(0, min(255, b))))
        }

        let homeFromCamera = alignment.session_to_home.matrix * frame.camera.transform
        let cameraFromHome = homeFromCamera.inverse
        struct Cell { var samples: Float = 0; var color = SIMD3<Float>(repeating: 0); var colored: Float = 0 }
        var cells = [SIMD3<Int32>: Cell]()
        // Every second pixel is under 4 cm apart at 5 m, finer than a voxel.
        for y in stride(from: 0, to: height, by: 2) {
            let depths = base.advanced(by: y * rowBytes).assumingMemoryBound(to: Float32.self)
            let confidences = confidenceBase.advanced(by: y * confidenceRow).assumingMemoryBound(to: UInt8.self)
            for x in stride(from: 0, to: width, by: 2) {
                guard confidences[x] >= UInt8(ARConfidenceLevel.high.rawValue),
                      let point = DepthProjection.point(x: x, y: y, depth: depths[x], width: width, height: height,
                          imageWidth: imageWidth, imageHeight: imageHeight, intrinsics: frame.camera.intrinsics) else { continue }
                let home = homeFromCamera * SIMD4(point[0], point[1], point[2], 1)
                let key = SIMD3(Int32((home.x / voxelSize).rounded(.down)), Int32((home.y / voxelSize).rounded(.down)),
                                Int32((home.z / voxelSize).rounded(.down)))
                var cell = cells[key] ?? Cell()
                cell.samples += 1
                if let rgb = linearColor(x: x, y: y) { cell.color += rgb; cell.colored += 1 }
                cells[key] = cell
            }
        }
        // One stray depth pixel does not make a voxel.
        var voxels = cells.compactMap { key, cell -> (point: SIMD3<Float>, color: [UInt8]?)? in
            guard cell.samples >= 2 else { return nil }
            let center = (SIMD3(Float(key.x), Float(key.y), Float(key.z)) + 0.5) * voxelSize
            let camera = cameraFromHome * SIMD4(center, 1)
            let point = SIMD3(camera.x, camera.y, camera.z)
            // Home rejects the whole capture if any point is outside 0.2-5 m of
            // camera depth. Snapping to a voxel center can move a point across
            // that limit, so apply Home's exact rule to the center itself.
            guard point.z <= -0.2, point.z >= -5, simd_length_squared(point) <= 64 else { return nil }
            let color: [UInt8]? = cell.colored > 0 ? {
                let mean = cell.color / cell.colored
                return [srgbByte(mean.x), srgbByte(mean.y), srgbByte(mean.z)]
            }() : nil
            return (point, color)
        }
        guard !voxels.isEmpty else { return nil }
        if voxels.count > maxVoxels {
            // Home accepts a bounded capture; keep the nearest, best-measured voxels.
            voxels.sort { simd_length_squared($0.point) < simd_length_squared($1.point) }
            voxels.removeLast(voxels.count - maxVoxels)
        }
        let colors = voxels.compactMap(\.color)
        return PhoneDepthSample(session: session, timestamp_ns: UInt64(frame.timestamp * 1_000_000_000),
            camera_to_session: .transform(frame.camera.transform, from: "phone-camera:\(session)", to: "phone-ar:\(session)"),
            alignment: alignment, points: voxels.map { [$0.point.x, $0.point.y, $0.point.z] },
            colors: colors.count == voxels.count ? colors : nil)
    }
}
