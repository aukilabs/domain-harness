import SwiftUI
import AVFoundation
import CoreLocation
import UserNotifications
import Network

/// The full-screen McKenna opening. On first boot the compass swings round
/// looking for north, then the screen lists what the app needs, one row per
/// permission, each asked for with its own tap. Every later boot shows the
/// mark briefly and goes straight on; screen recording, which iOS asks for
/// on every launch, is asked right after.
struct McKennaIntro: View {
    enum Mode { case firstBoot, quick }
    let mode: Mode
    let screen: ScreenShare
    /// Called once, when the app may start: screen recording has been asked
    /// for (or is skipped) and the camera may start.
    let done: () -> Void

    @State private var showList = false
    @State private var finished = false
    @StateObject private var permissions = Permissions()

    var body: some View {
        ZStack {
            Color(red: 0.05, green: 0.05, blue: 0.07).ignoresSafeArea()
            if showList {
                list.transition(.opacity)
            } else {
                mark.transition(.opacity)
            }
        }
        .preferredColorScheme(.dark)
        .task {
            try? await Task.sleep(nanoseconds: mode == .quick ? 900_000_000 : 2_600_000_000)
            if mode == .quick { finish() } else { withAnimation(.easeInOut(duration: 0.6)) { showList = true } }
        }
    }

    /// The badge inside a compass that swings round as if finding north, then settles.
    private var mark: some View {
        TimelineView(.animation) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let settle = mode == .quick ? 0.3 : 1.0
            let swing = (70 * sin(t * 1.7) + 25 * sin(t * 4.3)) * settle
            VStack(spacing: 22) {
                McKennaBadge()
                    .frame(width: 128, height: 128).clipShape(Circle())
                    .background { CompassRing(turn: swing).frame(width: 172, height: 172) }
                    .shadow(color: .black.opacity(0.4), radius: 18, y: 8)
                Text("McKenna").font(.system(size: 30, weight: .semibold, design: .rounded)).foregroundStyle(.white)
            }
        }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 14) {
                McKennaBadge().frame(width: 52, height: 52).clipShape(Circle())
                VStack(alignment: .leading, spacing: 3) {
                    Text("Before we start").font(.system(size: 22, weight: .semibold)).foregroundStyle(.white)
                    Text("McKenna needs a few things. Tap each one.").font(.system(size: 14)).foregroundStyle(.white.opacity(0.6))
                }
            }
            .padding(.bottom, 6)
            // One at a time, top to bottom: only the next row can be tapped.
            PermissionRow(symbol: "camera.fill", title: "Camera",
                          detail: "To read Portals and see the room in AR.",
                          state: permissions.gate(.camera)) { permissions.askCamera() }
            PermissionRow(symbol: "location.north.line.fill", title: "Location",
                          detail: "To find north and place the scene on the Earth.",
                          state: permissions.gate(.location)) { permissions.askLocation() }
            PermissionRow(symbol: "bell.badge.fill", title: "Notifications",
                          detail: "To tell you when someone leaves you a message.",
                          state: permissions.gate(.notifications)) { permissions.askNotifications() }
            PermissionRow(symbol: "wifi", title: "Local network",
                          detail: "To reach Home directly over this Wi-Fi.",
                          state: permissions.gate(.local)) { permissions.askLocalNetwork() }
            PermissionRow(symbol: "rectangle.dashed.badge.record", title: "Screen recording",
                          detail: "So Home can see this screen. iOS asks for this one every time McKenna opens.",
                          state: permissions.gate(.screen)) { permissions.askScreen(screen) }
            Spacer()
            Button(action: finish) {
                Text("Continue")
                    .font(.system(size: 17, weight: .semibold)).frame(maxWidth: .infinity).padding(.vertical, 14)
                    .background(Color.white, in: Capsule()).foregroundStyle(.black)
            }
            .disabled(!permissions.allAnswered)
            .opacity(permissions.allAnswered ? 1 : 0.35)
        }
        .padding(24)
        .onAppear { permissions.refresh() }
    }

    private func finish() {
        guard !finished else { return }
        finished = true
        // Never leave without the camera asked for: without it there is no AR at all.
        if mode == .firstBoot, AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined {
            AVCaptureDevice.requestAccess(for: .video) { _ in Task { @MainActor in done() } }
        } else {
            done()
        }
    }
}

/// One row: what it is, why, and its state or the button that asks for it.
private struct PermissionRow: View {
    let symbol: String
    let title: String
    let detail: String
    let state: Permissions.State
    let ask: () -> Void
    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: symbol).font(.system(size: 18, weight: .semibold)).foregroundStyle(.white)
                .frame(width: 40, height: 40).background(Color.white.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 16, weight: .semibold)).foregroundStyle(.white)
                Text(detail).font(.system(size: 12)).foregroundStyle(.white.opacity(0.55)).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            switch state {
            case .allowed:
                Image(systemName: "checkmark.circle.fill").font(.system(size: 22)).foregroundStyle(.green)
            case .waiting:
                Button("Allow", action: ask).font(.system(size: 14, weight: .semibold))
                    .padding(.horizontal, 14).padding(.vertical, 7).background(Color.white, in: Capsule()).foregroundStyle(.black)
            case .denied:
                Button("Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                }
                .font(.system(size: 14, weight: .semibold)).foregroundStyle(.orange)
            case .blocked:
                Text("Next").font(.system(size: 13)).foregroundStyle(.white.opacity(0.35))
            case .automatic:
                Text("Automatic").font(.system(size: 13)).foregroundStyle(.white.opacity(0.45))
            }
        }
    }
}

/// What each permission is at, and asking for it. iOS shows each system
/// prompt once; after a refusal only Settings can change it.
@MainActor
final class Permissions: NSObject, ObservableObject, CLLocationManagerDelegate {
    enum State { case waiting, allowed, denied, blocked, automatic }
    @Published var screen = State.waiting
    @Published var camera = State.waiting
    @Published var location = State.waiting
    @Published var notifications = State.waiting
    @Published var local = State.waiting
    private let locator = CLLocationManager()
    private var probe: NWConnection?

    enum Kind: CaseIterable { case camera, location, notifications, local, screen }
    private func state(_ kind: Kind) -> State {
        switch kind { case .camera: camera; case .location: location; case .notifications: notifications; case .local: local; case .screen: screen }
    }
    private func answered(_ kind: Kind) -> Bool { [.allowed, .denied].contains(state(kind)) }
    /// A row's state as shown: only the first unanswered row may be tapped.
    func gate(_ kind: Kind) -> State {
        let next = Kind.allCases.first { !answered($0) }
        return answered(kind) || kind == next ? state(kind) : .blocked
    }
    var allAnswered: Bool { Kind.allCases.allSatisfy(answered) }

    /// iOS asks about the local network the first time the app reaches a device on it,
    /// so open a connection to Home's own address, the way the app will later. The row
    /// is answered once that connects (allowed) or the prompt closes (the app is active again).
    func askLocalNetwork() {
        let url = UserDefaults.standard.string(forKey: "homeURL").flatMap(URL.init(string:))
            ?? URL(string: Demo.homeURL)!
        guard let host = url.host else { local = .allowed; return }
        let port = NWEndpoint.Port(rawValue: UInt16(url.port ?? 80)) ?? .http
        let connection = NWConnection(host: NWEndpoint.Host(host), port: port, using: .tcp)
        connection.stateUpdateHandler = { [weak self] state in
            if case .ready = state { Task { @MainActor in self?.answeredLocal(.allowed) } }
        }
        connection.start(queue: .main)
        probe = connection
        localWatch = NotificationCenter.default.addObserver(forName: UIApplication.didBecomeActiveNotification,
                                                            object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.answeredLocal(.allowed) }
        }
    }
    private var localWatch: NSObjectProtocol?
    private func answeredLocal(_ state: State) {
        guard probe != nil else { return }
        local = state
        probe?.cancel(); probe = nil
        if let localWatch { NotificationCenter.default.removeObserver(localWatch) }
        localWatch = nil
    }

    override init() {
        super.init()
        locator.delegate = self
    }

    func refresh() {
        camera = Self.state(AVCaptureDevice.authorizationStatus(for: .video))
        location = Self.state(locator.authorizationStatus)
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            let state: State = switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral: .allowed
            case .denied: .denied
            default: .waiting
            }
            Task { @MainActor in self.notifications = state }
        }
    }

    func askScreen(_ screen: ScreenShare) {
        screen.askFirst { allowed in Task { @MainActor in self.screen = allowed ? .allowed : .denied } }
    }
    func askCamera() {
        AVCaptureDevice.requestAccess(for: .video) { granted in
            Task { @MainActor in self.camera = granted ? .allowed : .denied }
        }
    }
    func askLocation() { locator.requestWhenInUseAuthorization() }
    func askNotifications() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, _ in
            Task { @MainActor in self.notifications = granted ? .allowed : .denied }
        }
    }

    nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        let state = Self.state(manager.authorizationStatus)
        Task { @MainActor in self.location = state }
    }

    nonisolated private static func state(_ status: AVAuthorizationStatus) -> State {
        switch status { case .authorized: .allowed; case .denied, .restricted: .denied; default: .waiting }
    }
    nonisolated private static func state(_ status: CLAuthorizationStatus) -> State {
        switch status { case .authorizedWhenInUse, .authorizedAlways: .allowed; case .denied, .restricted: .denied; default: .waiting }
    }
}
