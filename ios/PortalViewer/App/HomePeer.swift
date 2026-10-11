import Foundation
import UIKit
import SpatialPortalBridge

@MainActor
final class HomePeer: ObservableObject {
    @Published private(set) var depthDetail = "Depth not shared yet"
    @Published var email = ""
    @Published var password = ""
    @Published private(set) var profile: HomeProfile?
    @Published private(set) var running = false
    @Published private(set) var detail = "Pair with Home to connect"
    @Published private(set) var state = "stopped"
    @Published var annotationResult: AnnotationResult?
    struct AnnotationResult: Decodable, Equatable { let id: String; let saved: Bool; let detail: String }
    func saveAnnotation(_ placement: NotePlacement, text: String) -> Bool {
        guard state == "live" else { return false }
        var request = placement; request.text = text
        guard let data = try? JSONEncoder().encode(request), let json = String(data: data, encoding: .utf8) else { return false }
        annotationResult = nil
        return json.withCString { spatial_mobile_annotate($0) == 1 }
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
    var onMap: ((PortalMap) -> Void)?
    /// Home's answer to a new-Portal report, for the user to see.
    var onPortalResult: ((String) -> Void)?
    private var portalDetail = ""
    var onDisconnect: (() -> Void)?
    private var profileJSON = ""
    private var timer: Timer?
    private var lastVersion: UInt64 = 0
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

    init() {
        if let json = UserDefaults.standard.string(forKey: "homeConnectionProfile") {
            try? pair(Data(json.utf8))
        }
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
        guard !running, profile != nil, !email.isEmpty, !password.isEmpty else { return }
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
            let accepted = profileJSON.withCString { profile in
                email.withCString { user in
                    password.withCString { secret in
                        identity.withCString { path in spatial_mobile_start(profile, user, secret, path) }
                    }
                }
            }
            password = ""
            guard accepted == 1 else { throw ViewerError("Previous connection is still closing. Try again shortly.") }
            running = true; state = "connecting"; lastVersion = 0
            setName(UserDefaults.standard.string(forKey: "phoneName") ?? "")
            detail = "Signing in to Auki development services"
            onDisconnect?()
            timer?.invalidate()
            timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.poll() }
            }
        } catch { password = ""; detail = error.localizedDescription; state = "error" }
    }

    func disconnect(background: Bool = false) {
        guard running else { return }
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
    }
    private func poll() {
        guard let pointer = spatial_mobile_poll() else { return }
        defer { spatial_mobile_string_free(pointer) }
        do {
            let response = try JSONDecoder().decode(Poll.self, from: Data(String(cString: pointer).utf8))
            if response.update.version != lastVersion {
                lastVersion = response.update.version
                // User-requested Stop must not briefly redisplay an in-flight scene.
                if state != "stopping" {
                    depthDetail = response.update.depth_detail
                    if let note = response.update.local_detail, note != portalDetail, note.hasPrefix("Portal ") || note.hasPrefix("Home did not add") {
                        portalDetail = note
                        onPortalResult?(note)
                    }
                    annotationResult = response.update.annotation
                    state = response.update.state
                    detail = response.update.detail
                    if let portals = response.update.portals { onMap?(portals) }
                }
            }
            if response.finished {
                state = response.update.state; detail = response.update.detail
                running = false; timer?.invalidate(); timer = nil
                endBackgroundTask()
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
