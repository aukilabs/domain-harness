import Foundation
import UIKit
import Security
import SpatialPortalBridge
import CoreMedia
import Darwin

/// Sample the native host clock, not a frame's earlier capture time.
/// AR uses CLOCK_UPTIME_RAW, measured against the SDK session clock. Google's
/// GARFrame reference documents ARFrame timestamps as seconds since boot:
/// https://developers.google.com/ar/reference/ios/interface/GARFrame
/// Apple documents CLOCK_UPTIME_RAW as the nanosecond mach_absolute_time API:
/// https://developer.apple.com/documentation/kernel/1462446-mach_absolute_time
/// ReplayKit is sampled separately in Core Media host time. On-device epoch
/// and suspend/resume validation remains required; Rust rejects unmatched
/// captures and clock steps rather than assigning an arrival timestamp.
func sampleNativeCaptureClock(_ kind: UInt32) {
    let generation = spatial_mobile_clock_generation()
    guard generation > 0 else { return }
    let before = spatial_mobile_session_now(generation)
    guard before > 0 else { return }
    let native: UInt64
    if kind == 0 {
        native = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
    } else {
        let host = CMTimeConvertScale(CMClockGetTime(CMClockGetHostTimeClock()), timescale: 1_000_000_000, method: .default)
        guard host.isNumeric, host.value >= 0 else { return }
        native = UInt64(host.value)
    }
    let after = spatial_mobile_session_now(generation)
    _ = spatial_mobile_clock_sample(generation, kind, before, native, after)
}

@MainActor
final class HomePeer: ObservableObject {
    @Published private(set) var depthDetail = "Depth not shared yet"
    @Published var email = ""
    @Published var password = ""
    /// Sign in with an app key and secret instead of a User account.
    @Published var useAppKey = UserDefaults.standard.bool(forKey: "signInWithAppKey") {
        didSet { UserDefaults.standard.set(useAppKey, forKey: "signInWithAppKey") }
    }
    @Published var appKey = ""
    @Published var appSecret = ""
    @Published private(set) var savedAppKey = AppKeychain.load() != nil
    var hasCredentials: Bool {
        if useAppKey { return savedAppKey || (!appKey.isEmpty && !appSecret.isEmpty) }
        return !email.isEmpty && !password.isEmpty
    }
    func forgetAppKey() {
        AppKeychain.delete()
        savedAppKey = false
    }
    @Published private(set) var profile: HomeProfile?
    @Published private(set) var running = false
    @Published private(set) var detail = "Pair with Home to connect" { didSet { if detail != oldValue { print("[peer] \(state): \(detail)") } } }
    @Published private(set) var state = "stopped" {
        didSet { if state == "live" && oldValue != "live" { sendUnsent() } }
    }
    /// Notes named while Home was out of reach. They are sent once it is live again.
    private var unsent = [(placement: NotePlacement, text: String)]()
    /// Save to Home now if it is live; otherwise keep the note and sync it when Home is back.
    func saveOrQueue(_ placement: NotePlacement, text: String) {
        if !saveAnnotation(placement, text: text) { unsent.append((placement, text)) }
    }
    private func sendUnsent() {
        let waiting = unsent
        unsent = []
        for note in waiting { saveOrQueue(note.placement, text: note.text) }
    }
    @Published var annotationResult: AnnotationResult?
    struct AnnotationResult: Decodable, Equatable { let id: String; let saved: Bool; let detail: String }
    func saveAnnotation(_ placement: NotePlacement, text: String) -> Bool {
        guard state == "live" else { return false }
        if let at = placement.at {
            // Home takes a ready-made position as a note change, answered under the same id.
            let action: [String: Any] = ["kind": "place", "id": placement.id, "origin": placement.origin, "position": at, "text": text]
            let body: [String: Any] = ["request_id": placement.id, "frame": placement.frame, "action": action]
            guard let data = try? JSONSerialization.data(withJSONObject: body),
                  let json = String(data: data, encoding: .utf8) else { return false }
            return changeAnnotation(json)
        }
        var request = placement; request.text = text
        guard let data = try? JSONEncoder().encode(request), let json = String(data: data, encoding: .utf8) else { return false }
        annotationResult = nil
        return json.withCString { spatial_mobile_annotate($0) == 1 }
    }
    /// Ask Home to delete a note. Home answers in `annotationResult` under the returned id.
    private var deletingNotes = [String: String]()
    /// The note a delete request was for, once, when its answer comes.
    func takeDeletedNote(_ request: String) -> String? { deletingNotes.removeValue(forKey: request) }
    func deleteNote(_ id: String, frame: String) -> Bool {
        let request = UUID().uuidString.lowercased()
        let body: [String: Any] = ["request_id": request, "frame": frame, "action": ["kind": "delete", "id": id]]
        guard let data = try? JSONSerialization.data(withJSONObject: body),
              let json = String(data: data, encoding: .utf8), changeAnnotation(json) else { return false }
        deletingNotes[request] = id
        return true
    }
    func changeAnnotation(_ json: String) -> Bool {
        guard state == "live" else { return false }
        annotationResult = nil
        return json.withCString { spatial_mobile_change_note($0) == 1 }
    }
    func publishPose(_ sample: PhonePoseSample?) {
        guard let sample else { spatial_mobile_pose(nil); return }
        guard running, state == "live", let data = try? JSONEncoder().encode(sample),
              let json = String(data: data, encoding: .utf8) else { return }
        json.withCString { spatial_mobile_pose($0) }
    }
    /// The name Home shows for this phone. Sent on connect and when edited.
    func setName(_ name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        trimmed.withCString { spatial_mobile_name($0) }
    }
    /// Where this phone places the scene on the Earth. The bridge sends it on connect and when it changes.
    func setGeo(_ geo: HomeGeo) {
        guard let data = try? JSONEncoder().encode(geo), let json = String(data: data, encoding: .utf8) else { return }
        json.withCString { spatial_mobile_geo($0) }
    }
    /// Hold the joystick for one robot, or release it with a nil target. The
    /// bridge repeats the held value and sends explicit stops on release.
    func drive(target: String?, forward: Float = 0, turn: Float = 0) {
        guard let target, state == "live" else { spatial_mobile_drive(nil, 0, 0); return }
        target.withCString { spatial_mobile_drive($0, forward, turn) }
    }
    /// Ask Home to add a newly seen Portal to the scene.
    @discardableResult
    func reportPortal(_ report: PortalReport) -> Bool {
        guard state == "live", let data = try? JSONEncoder().encode(report),
              let json = String(data: data, encoding: .utf8) else { return false }
        return json.withCString { spatial_mobile_report_portal($0) == 1 }
    }
    /// Tell Home the camera is paused; it keeps the last pose, marked asleep.
    func setAsleep(_ asleep: Bool) { spatial_mobile_sleep(asleep ? 1 : 0) }
    func publishDepth(_ sample: PhoneDepthSample?) {
        guard let sample else { spatial_mobile_depth(nil); return }
        guard running, state == "live", let data = try? JSONEncoder().encode(sample),
              let json = String(data: data, encoding: .utf8) else { return }
        json.withCString { spatial_mobile_depth($0) }
    }
    /// Publish where the AR session sits in the scene; nil says the phone no longer knows.
    /// Kept so that a peer started later publishes it too.
    func publishPlacement(_ placement: PhonePlacement?) {
        self.placement = placement
        guard let placement else { spatial_mobile_placement(nil); return }
        guard let data = try? JSONEncoder().encode(placement), let json = String(data: data, encoding: .utf8) else { return }
        json.withCString { spatial_mobile_placement($0) }
    }
    private var placement: PhonePlacement?
    /// Share the phone's own map with Home; nil stops sharing it.
    func publishLocalMap(_ map: PhoneLocalMap?) {
        guard let map else { _ = spatial_mobile_local_map(nil); return }
        guard running, state == "live", let data = try? JSONEncoder().encode(map),
              let json = String(data: data, encoding: .utf8) else { return }
        json.withCString { _ = spatial_mobile_local_map($0) }
    }
    /// Save a drawn path to Home, or remove a saved one, over the peer link.
    /// `done` hears whether Home did it; no answer in six seconds counts as no.
    func saveStroke(_ stroke: CrayonStroke, frame: String, done: @escaping (Bool) -> Void) {
        struct Add: Encodable { let frame: String; let stroke: CrayonStroke }
        changePath(stroke.id, Add(frame: frame, stroke: stroke), done: done)
    }
    func eraseStroke(_ id: String, frame: String, done: @escaping (Bool) -> Void) {
        struct Remove: Encodable { let frame: String; let id: String }
        changePath(id, Remove(frame: frame, id: id), done: done)
    }
    private var pathAnswers = [String: (Bool) -> Void]()
    private func changePath(_ id: String, _ change: some Encodable, done: @escaping (Bool) -> Void) {
        guard state == "live", let data = try? JSONEncoder().encode(change), let json = String(data: data, encoding: .utf8),
              json.withCString({ spatial_mobile_change_path($0) == 1 }) else { print("[crayon] Path \(id) not sent: Home is not connected, or too many changes are waiting"); done(false); return }
        pathAnswers[id] = done
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) { [weak self] in
            guard let done = self?.pathAnswers.removeValue(forKey: id) else { return }
            print("[crayon] Path \(id): no answer from Home in 6 s"); done(false)
        }
    }
    /// Ask Home to plan a route in the scene. With `share` it replaces this phone's route on
    /// everyone's map. `done` hears nil when Home cannot plan routes or does not answer.
    func planRoute(frame: String, from: RoutePlace, to: RoutePlace, share: Bool, done: @escaping (HomeRoute?) -> Void) {
        struct Plan: Encodable { let frame: String; let from: RoutePlace; let to: RoutePlace; let share: Bool }
        askRoute(Plan(frame: frame, from: from, to: to, share: share), done: done)
    }
    /// Take this phone's route off everyone's map.
    func clearRoute(frame: String) {
        struct Clear: Encodable { let frame: String }
        askRoute(Clear(frame: frame)) { _ in }
    }
    private var routeAnswers = [String: (HomeRoute?) -> Void]()
    private struct Asked<Ask: Encodable>: Encodable { let id: String; let ask: Ask }
    private func askRoute<Ask: Encodable>(_ ask: Ask, done: @escaping (HomeRoute?) -> Void) {
        let id = UUID().uuidString.lowercased()
        guard state == "live", let data = try? JSONEncoder().encode(Asked(id: id, ask: ask)), let json = String(data: data, encoding: .utf8),
              json.withCString({ spatial_mobile_ask_route($0) == 1 }) else { done(nil); return }
        routeAnswers[id] = done
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) { [weak self] in self?.routeAnswers.removeValue(forKey: id)?(nil) }
    }
    // MARK: Onboarding (onboarding.md)
    /// The onboarding this phone hosts or takes part in, as the bridge reports it.
    @Published private(set) var onboarding: OnboardingState?
    /// A scanned invitation waiting for this phone to reach the invitation's Home.
    private var pendingInvitation: String?
    private var switchingTo: Data?
    /// Start showing the way into this phone's scene. Answers the invitation QR text.
    func hostOnboarding(_ request: [String: Any]) async -> Result<String, ViewerError> {
        // Reserve before dispatch: Cancel or a newer attempt can invalidate a
        // worker even if it has not started on the background queue yet.
        let prepared = Onboarding.call { "{\"prepare\":true}".withCString { spatial_mobile_onboarding_host($0) } }
        let attempt: String
        switch prepared {
        case .failure(let error): return .failure(error)
        case .success(let value):
            guard let id = value["attempt"] as? String else { return .failure(ViewerError("Could not prepare onboarding")) }
            attempt = id
        }
        var reserved = request
        reserved["attempt"] = attempt
        guard let data = try? JSONSerialization.data(withJSONObject: reserved),
              let json = String(data: data, encoding: .utf8) else { return .failure(ViewerError("Invalid onboarding request")) }
        return await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let result = Onboarding.call { json.withCString { spatial_mobile_onboarding_host($0) } }.flatMap { value -> Result<String, ViewerError> in
                    guard let text = value["invitation"] as? String else { return .failure(ViewerError("No invitation")) }
                    return .success(text)
                }
                continuation.resume(returning: result)
            }
        }
    }
    func stopOnboarding() {
        pendingInvitation = nil
        spatial_mobile_onboarding_stop()
    }
    /// An invitation QR seen during normal scanning. Joins its Home first if
    /// this phone is on another one. Repeat scans of one session do nothing.
    func handleInvitation(_ text: String) {
        guard pendingInvitation != text else { return }
        guard case .success(let value) = Onboarding.call({ text.withCString { spatial_mobile_onboarding_parse($0) } }),
              let home = value["home"] as? [String: Any], let peerID = home["peer_id"] as? String,
              let frame = home["frame"] as? String,
              let homeData = try? JSONSerialization.data(withJSONObject: home) else { return }
        if profile?.peer_id == peerID && profile?.frame == frame && state == "live" {
            join(text)
            return
        }
        print("[onboarding] Invitation for \(frame); connecting to that Home first")
        pendingInvitation = text
        if running {
            if profile?.peer_id != peerID || profile?.frame != frame { switchingTo = homeData; disconnect() }
        } else {
            do { try pair(homeData); connect() } catch { pendingInvitation = nil; detail = error.localizedDescription }
        }
    }
    private func joinPending() {
        guard let text = pendingInvitation else { return }
        pendingInvitation = nil
        join(text)
    }
    private func join(_ text: String) {
        switch Onboarding.call({ text.withCString { spatial_mobile_onboarding_join($0) } }) {
        case .success(let value): if value["started"] as? Bool == true { print("[onboarding] Joining") }
        case .failure(let error): print("[onboarding] Not joined: \(error.message)")
        }
    }

    var onMap: ((PortalMap) -> Void)?
    /// Voxel chunks that changed, as the bridge hands them over.
    var onVoxels: ((Data) -> Void)?
    /// Home's answer to a new-Portal report, for the user to see.
    var onPortalResult: ((String) -> Void)?
    private var portalDetail = ""
    var onDisconnect: (() -> Void)?
    private var profileJSON = ""
    private var timer: Timer?
    private var lastVersion: UInt64 = 0
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

    // Local addition: the last login that reached Home is kept in the device
    // Keychain so the app can connect and start matching Portals on launch.
    @Published private(set) var hasSavedLogin = false
    private var pendingLogin: SavedLogin.Entry?
    /// A sign-in ended without reaching Home, so the sign-in page should open.
    @Published private(set) var needsSignIn = false
    private var reachedHome = false
    private var stopRequested = false
    private var reconnectWhenFinished = false

    init() {
        if let json = UserDefaults.standard.string(forKey: "homeConnectionProfile") {
            try? pair(Data(json.utf8))
        }
        // Development convenience: a launch environment profile replaces the pairing.
        if let json = ProcessInfo.processInfo.environment["AUKI_HOME_PROFILE"], !json.isEmpty {
            do { try pair(Data(json.utf8)); print("[peer] paired from launch environment"); profileFresh = true }
            catch { print("[peer] launch profile rejected: \(error.localizedDescription)") }
        }
        // Home's web address is kept so later launches can ask Home for its current scene.
        if let url = ProcessInfo.processInfo.environment["AUKI_HOME_URL"], !url.isEmpty {
            UserDefaults.standard.set(url, forKey: "homeURL")
        } else if UserDefaults.standard.string(forKey: "homeURL") == nil {
            UserDefaults.standard.set(Demo.homeURL, forKey: "homeURL")
        }
        if let saved = SavedLogin.load() { email = saved.email; hasSavedLogin = true }
    }

    /// Connect with the saved login. Returns false when there is nothing to use.
    @discardableResult
    func autoConnect() -> Bool {
        // Nothing touches the network until the permission list is done: the
        // first packet raises iOS's Local Network prompt over it.
        guard permissionsAsked else { return true }
        // Development convenience: a launch environment login is used when no
        // Keychain login exists. It is saved to the Keychain once Home is reached.
        let environment = ProcessInfo.processInfo.environment
        let launchLogin: SavedLogin.Entry = {
            guard let email = environment["AUKI_EMAIL"], let password = environment["AUKI_PASSWORD"],
                  !email.isEmpty, !password.isEmpty else { return SavedLogin.Entry(email: Demo.email, password: Demo.password) }
            return SavedLogin.Entry(email: email, password: password)
        }()
        if running {
            // A background disconnect may still be closing; reconnect when it ends.
            if state == "stopping" { reconnectWhenFinished = true }
            return true
        }
        // Never paired: ask the demo Home for its current scene first.
        if profile == nil, !askedHomeForProfile, let home = UserDefaults.standard.string(forKey: "homeURL").flatMap(URL.init(string:)) {
            askedHomeForProfile = true
            refreshProfile(from: home)
            return true
        }
        // The app key from the sign-in page, when that is how this phone signs in.
        if profile != nil, useAppKey, savedAppKey { connect(); return running }
        guard profile != nil, !useAppKey, let saved = Optional(SavedLogin.load() ?? launchLogin) else { print("[peer] autoConnect: profile=\(profile != nil) no login available"); return false }
        email = saved.email; password = saved.password
        // A deleted and recreated scenegraph has a new epoch, so the saved profile may
        // name a scene Home no longer has. Ask Home for its current one first.
        if !profileFresh, let home = UserDefaults.standard.string(forKey: "homeURL").flatMap(URL.init(string:)) {
            refreshProfile(from: home)
            return true
        }
        profileFresh = false  // the next reconnect asks Home again
        connect()
        return running
    }

    private var askedHomeForProfile = false
    /// The permission list is done, so reaching Home cannot raise the Local
    /// Network prompt over it.
    var permissionsAsked = false
    /// The profile came from Home just now, at launch or from refreshProfile.
    private var profileFresh = false
    private var refreshing = false

    private func refreshProfile(from home: URL) {
        guard !refreshing else { return }
        refreshing = true
        var request = URLRequest(url: home.appendingPathComponent("api/home-connection"))
        request.timeoutInterval = 4
        URLSession.shared.dataTask(with: request) { data, response, _ in
            DispatchQueue.main.async {
                self.refreshing = false
                self.profileFresh = true
                if let data, (response as? HTTPURLResponse)?.statusCode == 200, !self.running,
                   String(decoding: data, as: UTF8.self) != self.profileJSON {
                    do { try self.pair(data); print("[peer] paired to Home's current scene") }
                    catch { print("[peer] Home's current profile rejected: \(error.localizedDescription)") }
                } else if data == nil, self.profile == nil {
                    // Never paired: keep asking until Home answers.
                    print("[peer] Home not reachable yet; asking again in 3 s")
                    self.askedHomeForProfile = false
                    DispatchQueue.main.asyncAfter(deadline: .now() + 3) { self.autoConnect() }
                    return
                } else if data == nil {
                    print("[peer] Home not reachable for a fresh profile; using the saved one")
                } else {
                    print("[peer] Home's current scene matches the saved profile")
                }
                self.autoConnect()
            }
        }.resume()
    }

    func forgetLogin() {
        SavedLogin.clear(); hasSavedLogin = false; pendingLogin = nil; reconnectWhenFinished = false
    }

    func pair(_ data: Data) throws {
        guard !running else { throw ViewerError("Disconnect before pairing another Home") }
        guard data.count <= 32 * 1024 else { throw ViewerError("Home profile is too large") }
        let value = try HomeProfile.load(data)
        profileJSON = String(decoding: data, as: UTF8.self)
        profile = value
        // Public pairing information only. Password and peer key never enter defaults.
        UserDefaults.standard.set(profileJSON, forKey: "homeConnectionProfile")
        detail = "Home paired · sign in to connect"
        onDisconnect?()
    }

    // Public pairing files transferred over USB are offered explicitly, never
    // silently substituted for a user's saved connection.
    var transferredConnections: [URL] {
        guard let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first,
              let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.fileSizeKey]) else { return [] }
        return files.filter { url in
            guard url.lastPathComponent.hasPrefix("home-connection-"), url.pathExtension == "json",
                  let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size <= 32 * 1024,
                  let data = try? Data(contentsOf: url) else { return false }
            return (try? HomeProfile.load(data)) != nil
        }.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }
    func transferredConnectionName(_ url: URL) -> String {
        guard let data = try? Data(contentsOf: url), let profile = try? HomeProfile.load(data) else { return url.lastPathComponent }
        return profile.sceneLabel
    }

    func connect() {
        guard !running, profile != nil, hasCredentials else { return }
        // Newly typed app credentials replace any saved ones.
        let typed = !appKey.isEmpty && !appSecret.isEmpty
        let app = useAppKey ? (typed ? AppKeychain.Credentials(key: appKey, secret: appSecret) : AppKeychain.load()) : nil
        if useAppKey && app == nil { savedAppKey = false; detail = "Enter the app key and secret"; return }
        do {
            let manager = FileManager.default
            var directory = try manager.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                             appropriateFor: nil, create: true).appendingPathComponent("AukiPeer", isDirectory: true)
            try manager.createDirectory(at: directory, withIntermediateDirectories: true,
                                        attributes: [.protectionKey: FileProtectionType.complete])
            var values = URLResourceValues(); values.isExcludedFromBackup = true
            try directory.setResourceValues(values)
            let identity = directory.appendingPathComponent("phone.identity").path
            if manager.fileExists(atPath: identity) {
                try manager.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: identity)
            }
            let (first, second) = app.map { ($0.key, $0.secret) } ?? (email, password)
            print("[bridge] built from \(String(cString: spatial_bridge_source()))")
            // Home dials this phone directly at its Wi-Fi address; the relay stays as a fallback.
            if let address = Self.wifiAddress() {
                address.withCString { spatial_mobile_set_lan_address($0) }
            } else {
                spatial_mobile_set_lan_address(nil)
            }
            let start = app == nil ? spatial_mobile_start : spatial_mobile_start_app
            let accepted = profileJSON.withCString { profile in
                first.withCString { first in
                    second.withCString { second in
                        identity.withCString { path in start(profile, first, second, path) }
                    }
                }
            }
            pendingLogin = app == nil ? SavedLogin.Entry(email: email, password: password) : nil
            reachedHome = false; stopRequested = false; needsSignIn = false
            password = ""
            guard accepted == 1 else { throw ViewerError("Previous connection is still closing. Try again shortly.") }
            if let app, typed {
                savedAppKey = AppKeychain.save(app)
                appKey = ""; appSecret = ""
            }
            running = true; state = "connecting"; lastVersion = 0
            setName(UserDefaults.standard.string(forKey: "phoneName") ?? "")
            detail = "Signing in to Auki development services"
            onDisconnect?()
            timer?.invalidate()
            timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.poll() }
            }
        } catch { password = ""; appSecret = ""; detail = error.localizedDescription; state = "error" }
    }

    /// This phone's Wi-Fi IPv4 address, if it is on Wi-Fi.
    static func wifiAddress() -> String? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        for entry in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let interface = entry.pointee
            guard String(cString: interface.ifa_name) == "en0", let address = interface.ifa_addr,
                  address.pointee.sa_family == UInt8(AF_INET) else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            if getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count),
                           nil, 0, NI_NUMERICHOST) == 0 {
                return String(cString: host)
            }
        }
        return nil
    }

    func disconnect(background: Bool = false) {
        guard running else { return }
        reconnectWhenFinished = false
        stopRequested = true
        if background && backgroundTask == .invalid {
            backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Close Home peer") { [weak self] in
                Task { @MainActor in self?.endBackgroundTask() }
            }
        }
        spatial_mobile_stop()
        state = "stopping"; detail = "Disconnecting from Home"
        onDisconnect?()
    }

    private struct Poll: Decodable {
        let finished: Bool
        let update: Update
    }
    private struct Update: Decodable {
        let version: UInt64
        let state: String
        let detail: String
        let portals: PortalMap?
        let annotation: AnnotationResult?
        let depth_detail: String
        let local_detail: String?
        let route: RouteAnswer?
        let paths: [PathAnswer]?
        let onboarding: OnboardingState?
    }
    private struct PathAnswer: Decodable {
        let id: String
        let done: Bool
        let error: String?
    }
    private struct RouteAnswer: Decodable {
        let id: String
        let route: HomeRoute?
        let error: String?
    }
    private func poll() {
        if let voxels = spatial_mobile_voxels() {
            let data = Data(String(cString: voxels).utf8)
            spatial_mobile_string_free(voxels)
            onVoxels?(data)
        }
        guard let pointer = spatial_mobile_poll() else { return }
        defer { spatial_mobile_string_free(pointer) }
        do {
            let response = try JSONDecoder().decode(Poll.self, from: Data(String(cString: pointer).utf8))
            if response.update.version != lastVersion {
                lastVersion = response.update.version
                // User-requested Stop must not briefly redisplay an in-flight scene.
                if state != "stopping" {
                    depthDetail = response.update.depth_detail
                    for answer in response.update.paths ?? [] {
                        guard let done = pathAnswers.removeValue(forKey: answer.id) else { continue }
                        if let error = answer.error { print("[crayon] Path \(answer.id) not changed: \(error)") }
                        done(answer.done)
                    }
                    if let note = response.update.local_detail, note != portalDetail, note.hasPrefix("Portal ") || note.hasPrefix("Home did not add") {
                        portalDetail = note
                        onPortalResult?(note)
                    }
                    if let answer = response.update.route, let done = routeAnswers.removeValue(forKey: answer.id) {
                        if let error = answer.error { print("[route] \(error)") }
                        done(answer.route)
                    }
                    annotationResult = response.update.annotation
                    if onboarding != response.update.onboarding {
                        if let o = response.update.onboarding, o.detail != onboarding?.detail || o.stage != onboarding?.stage {
                            print("[onboarding] \(o.role) \(o.stage ?? "starting") · \(o.detail)")
                        }
                        onboarding = response.update.onboarding
                    }
                    // A peer started after the phone aligned has not been told where it is.
                    if response.update.state == "live", state != "live", let placement { publishPlacement(placement) }
                    state = response.update.state
                    detail = response.update.detail
                    if let portals = response.update.portals { onMap?(portals) }
                    if state == "live" { reachedHome = true; joinPending() }
                    if state == "live" || detail.hasPrefix("Authorizing"), let login = pendingLogin {
                        hasSavedLogin = SavedLogin.save(login); pendingLogin = nil
                    }
                }
            }
            if response.finished {
                state = response.update.state; detail = response.update.detail
                running = false; timer?.invalidate(); timer = nil
                endBackgroundTask()
                pendingLogin = nil
                if !reachedHome && !stopRequested && detail.localizedCaseInsensitiveContains("sign-in failed") { needsSignIn = true }
                if let switching = switchingTo {
                    // An invitation for another Home: pair with it, then join once live.
                    switchingTo = nil
                    do { try pair(switching); autoConnect() } catch { pendingInvitation = nil; detail = error.localizedDescription }
                } else if reconnectWhenFinished { reconnectWhenFinished = false; autoConnect() }
            }
        } catch {
            spatial_mobile_stop()
            state = "stopping"
            detail = "Invalid bridge response. Closing connection."
        }
    }

    private func endBackgroundTask() {
        if backgroundTask != .invalid {
            UIApplication.shared.endBackgroundTask(backgroundTask)
            backgroundTask = .invalid
        }
    }
}

/// Demo build: the shared demo login and Nils's Home, so a plain Xcode
/// Cmd-R or install opens signed in. Launch environment values override.
enum Demo {
    static let homeURL = "http://192.168.1.26:4329"
    static let email = "dep.monaghan@gmail.com"
    static let password = "bynny6-Fohpit-fohkes"
}

/// The app key and secret, kept only in this iPhone's Keychain: never in defaults,
/// backups or other devices.
enum AppKeychain {
    struct Credentials: Codable { let key: String; let secret: String }
    private static let item: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "com.spatialharness.PortalViewer.app-key",
        kSecAttrAccount as String: "auki-app",
    ]

    static func load() -> Credentials? {
        var query = item
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data else { return nil }
        return try? JSONDecoder().decode(Credentials.self, from: data)
    }

    static func save(_ credentials: Credentials) -> Bool {
        guard let data = try? JSONEncoder().encode(credentials) else { return false }
        SecItemDelete(item as CFDictionary)
        var query = item
        query[kSecValueData as String] = data
        query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(query as CFDictionary, nil) == errSecSuccess
    }

    static func delete() { SecItemDelete(item as CFDictionary) }
}

/// Device-only Keychain item; not synced and unreadable while the phone is locked.
enum SavedLogin {
    struct Entry: Codable { let email: String; let password: String }
    private static let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "home-portal-viewer.auki-user",
        kSecAttrAccount as String: "login",
    ]
    static func load() -> Entry? {
        var q = query; q[kSecReturnData as String] = true; q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return try? JSONDecoder().decode(Entry.self, from: data)
    }
    @discardableResult
    static func save(_ entry: Entry) -> Bool {
        guard let data = try? JSONEncoder().encode(entry) else { return false }
        SecItemDelete(query as CFDictionary)
        var q = query
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
    }
    static func clear() { SecItemDelete(query as CFDictionary) }
}
