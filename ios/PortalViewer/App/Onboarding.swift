import SwiftUI
import AudioToolbox
import SpatialPortalBridge

/// Onboarding (onboarding.md): this phone shows another device, a phone or a
/// robot, the way into its scene, or joins a scene by watching another phone.
/// The bridge does the work; this file is the screens.
enum Onboarding {
    /// What an invitation QR starts with.
    static let prefix = #"{"schema":"spatial-harness.onboarding/v1""#

    /// This phone's model identifier, e.g. "iPhone16,1".
    static var model: String {
        var system = utsname()
        uname(&system)
        return withUnsafeBytes(of: &system.machine) { bytes in
            String(decoding: bytes.prefix(while: { $0 != 0 }), as: UTF8.self)
        }
    }

    /// Call a bridge function that answers JSON, or `{"error": ...}`.
    static func call(_ body: () -> UnsafeMutablePointer<CChar>?) -> Result<[String: Any], ViewerError> {
        guard let pointer = body() else { return .failure(ViewerError("The bridge did not answer")) }
        defer { spatial_mobile_string_free(pointer) }
        guard let value = try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any] else {
            return .failure(ViewerError("The bridge answered something unreadable"))
        }
        if let error = value["error"] as? String { return .failure(ViewerError(error)) }
        return .success(value)
    }

    /// The marker's side on this phone's screen in points, or why this phone cannot show one.
    static func markerSide() -> Result<CGFloat, ViewerError> {
        call { model.withCString { spatial_mobile_onboarding_screen($0) } }.flatMap { value in
            guard let side = value["side_points"] as? Double else { return .failure(ViewerError("No marker size for this phone")) }
            return .success(CGFloat(side))
        }
    }

    struct Code { let width: Int; let dark: [Bool] }
    static func code(_ text: String) -> Code? {
        guard case .success(let value) = call({ text.withCString { spatial_mobile_onboarding_qr($0) } }),
              let width = value["width"] as? Int, let modules = value["modules"] as? String,
              modules.count == width * width else { return nil }
        return Code(width: width, dark: modules.map { $0 == "1" })
    }

    static func playSuccess() {
        AudioServicesPlaySystemSound(1016)
        UINotificationFeedbackGenerator().notificationOccurred(.success)
    }
}

/// Onboarding as the bridge reports it.
struct OnboardingState: Decodable, Equatable {
    let role: String
    let session: String
    let alias: String
    let invitation: String?
    let marker: String?
    let joiner: String?
    let stage: String?
    let detail: String
    let looking: Bool?
    let sightings: Int?
    let clock_uncertainty_ms: Double?
    let alignment: JoinAlignment?
    struct JoinAlignment: Codable, Equatable {
        let timestamp_ns: UInt64
        let portal_id: String
        let portal_to_session: Portal.Pose
        let session_to_home: Portal.Pose
    }
}

/// A QR code drawn module by module at an exact size, quiet zone excluded.
struct ModuleCode: View {
    let code: Onboarding.Code
    let side: CGFloat
    var body: some View {
        Canvas { context, _ in
            let module = side / CGFloat(code.width)
            var dark = Path()
            for row in 0..<code.width {
                for column in 0..<code.width where code.dark[row * code.width + column] {
                    dark.addRect(CGRect(x: CGFloat(column) * module, y: CGFloat(row) * module, width: module, height: module))
                }
            }
            context.fill(dark, with: .color(.black))
        }
        .frame(width: side, height: side)
    }
}

/// The host's screens: alias, calibration, the invitation, the marker, success.
struct OnboardingHostView: View {
    @ObservedObject var controller: ViewerController
    @ObservedObject var peer: HomePeer
    let close: () -> Void
    @State private var alias = ""
    @State private var ssid = ""
    @State private var wifiPassword = ""
    @State private var withWifi = false
    @State private var asked = false
    @State private var problem: String?
    @State private var invitation: Onboarding.Code?
    @State private var marker: Onboarding.Code?
    @State private var markerSide: CGFloat = 0
    @State private var succeeded = false
    @State private var brightness: CGFloat?

    private var state: OnboardingState? { peer.onboarding?.role == "host" ? peer.onboarding : nil }
    private var showingMarker: Bool { ["marker", "measuring"].contains(state?.stage ?? "") }

    var body: some View {
        ZStack {
            if !asked {
                askAlias
            } else if state == nil && !succeeded {
                calibrating
            } else {
                // The camera stays in view around the code, so whoever holds this
                // phone can see where they are going. The code sits on its own white
                // card with a four-module quiet zone, fixed in the middle of the
                // screen: its place there is part of the geometry.
                GeometryReader { _ in
                    let bounds = UIScreen.main.bounds
                    if succeeded {
                        done.position(x: bounds.midX, y: bounds.midY)
                    } else if showingMarker, let marker {
                        card(marker, side: markerSide).position(x: bounds.midX, y: bounds.midY)
                    } else if let invitation {
                        card(invitation, side: min(bounds.width, bounds.height) * 0.78).position(x: bounds.midX, y: bounds.midY)
                    }
                }
                .ignoresSafeArea()
                VStack {
                    VStack(spacing: 4) {
                        Text(caption).themeLabel(16, .medium).multilineTextAlignment(.center)
                        if let detail = state?.detail, showingMarker {
                            Text(detail).themeText(11).foregroundStyle(Theme.dim).multilineTextAlignment(.center)
                        }
                        if let problem { Text(problem).themeText(11).foregroundStyle(Theme.accent).multilineTextAlignment(.center) }
                    }
                    .frame(maxWidth: .infinity).themePanel(padding: 14).padding(.top, 50).padding(.horizontal, 16)
                    Spacer()
                    Button("Cancel") { finish() }.buttonStyle(PillButtonStyle(filled: true)).padding(.bottom, 40)
                }
            }
        }
        .onChange(of: controller.calibrated) { _, calibrated in if calibrated && asked && state == nil { start() } }
        .onChange(of: state?.stage) { _, stage in
            if stage == "aligned", !succeeded {
                succeeded = true
                Onboarding.playSuccess()
                DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { finish() }
            }
        }
        .onChange(of: state?.marker) { _, payload in marker = payload.flatMap(Onboarding.code) }
        .onDisappear { release() }
    }

    private var caption: String {
        if succeeded { return "\(alias) is in the scene" }
        if showingMarker { return "Hold this phone still, screen toward \(alias)" }
        return "Display marker to device to begin onboarding"
    }

    private var askAlias: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Add a device").themeLabel(17, .medium)
            Text("A name for the robot or phone you are adding.").themeText(11).foregroundStyle(Theme.dim)
            TextField("Alias", text: $alias).textFieldStyle(SoftFieldStyle()).autocorrectionDisabled()
                .textInputAutocapitalization(.words).submitLabel(.next).onSubmit(next)
            Toggle("It needs to join a Wi-Fi network", isOn: $withWifi).toggleStyle(CheckToggleStyle())
            if withWifi {
                TextField("Wi-Fi name", text: $ssid).textFieldStyle(SoftFieldStyle()).autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                SecureField("Wi-Fi password", text: $wifiPassword).textFieldStyle(SoftFieldStyle())
            }
            if let problem { Text(problem).themeText(11).foregroundStyle(Theme.accent) }
            HStack {
                Button("Cancel") { finish() }
                Spacer()
                Button("Next", action: next).disabled(alias.trimmingCharacters(in: .whitespaces).isEmpty)
            }.buttonStyle(PillButtonStyle())
        }
        .themePanel()
        .frame(maxHeight: .infinity, alignment: .top).padding(.top, 60).padding(.horizontal, 16)
    }

    private var calibrating: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Calibrate this phone").themeLabel(17, .medium)
            Text("Point the camera at a Portal of this scene. The code for \(alias) appears as soon as this phone knows where it is.")
                .themeText(11).foregroundStyle(Theme.dim)
            if let problem { Text(problem).themeText(11).foregroundStyle(Theme.accent) }
            HStack { Spacer(); Button("Cancel") { finish() }.buttonStyle(PillButtonStyle()) }
        }
        .themePanel()
        .frame(maxHeight: .infinity, alignment: .top).padding(.top, 60).padding(.horizontal, 16)
    }

    private func card(_ code: Onboarding.Code, side: CGFloat) -> some View {
        ModuleCode(code: code, side: side)
            .padding(side / CGFloat(code.width) * 4)
            .background(Color.white)
    }

    private var done: some View {
        Image(systemName: "checkmark.circle.fill").font(.system(size: 120)).foregroundStyle(.green)
    }

    private func next() {
        guard !alias.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        if withWifi && (ssid.isEmpty || (!wifiPassword.isEmpty && wifiPassword.count < 8)) {
            problem = "Enter the Wi-Fi name, and a password of at least 8 characters if it has one"; return
        }
        problem = nil
        asked = true
        controller.onboardingHosting = true
        if controller.calibrated { start() }
    }

    private func start() {
        switch Onboarding.markerSide() {
        case .failure(let error): problem = error.message; return
        case .success(let side): markerSide = side
        }
        let bounds = UIScreen.main.bounds
        let wifi: [String: Any]? = withWifi ? ["ssid": ssid].merging(wifiPassword.isEmpty ? [:] : ["password": wifiPassword]) { a, _ in a } : nil
        var request: [String: Any] = ["alias": alias.trimmingCharacters(in: .whitespaces),
            "display": ["model": Onboarding.model, "center_points": [bounds.midX, bounds.midY], "side_points": markerSide]]
        if let wifi { request["wifi"] = wifi }
        switch peer.hostOnboarding(request) {
        case .failure(let error): problem = error.message
        case .success(let text):
            problem = nil
            invitation = Onboarding.code(text)
            if brightness == nil { brightness = UIScreen.main.brightness }
            UIScreen.main.brightness = 1
        }
    }

    private func release() {
        controller.onboardingHosting = false
        if let brightness { UIScreen.main.brightness = brightness }
        brightness = nil
    }

    private func finish() {
        peer.stopOnboarding()
        release()
        close()
    }
}

/// The joiner's progress, over the camera view.
struct OnboardingJoinCard: View {
    let state: OnboardingState
    let cancel: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(state.stage == "aligned" ? "Joined as \(state.alias)" : "Joining as \(state.alias)").themeLabel(15, .medium)
            Text(state.detail).themeText(11).foregroundStyle(state.stage == "failed" ? Theme.accent : Theme.dim)
            if state.stage != "aligned" {
                HStack { Spacer(); Button(state.stage == "failed" ? "Close" : "Cancel", action: cancel).buttonStyle(PillButtonStyle()) }
            }
        }
        .themePanel()
    }
}
