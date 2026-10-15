import Foundation
import ReplayKit
import CoreImage
import SpatialPortalBridge

/// The app's own screen for Home, captured with ReplayKit only while Home is
/// subscribed to it. Home opening the screen in its picture-in-picture is the
/// request; closing it ends it. iOS asks the person before the first capture.
final class ScreenShare: ObservableObject {
    /// True while the screen is being captured for someone.
    @Published private(set) var sharing = false

    /// Height of the picture sent (the app is portrait); its width keeps the screen's shape.
    private static let height = 640.0
    private static let interval = 0.125
    /// Keep capturing this long after demand goes, so a reader that
    /// resubscribes does not cost another start.
    private static let linger = 3.0
    /// After iOS refused (or the person declined), ask again no sooner than this.
    private static let retryAfter = 30.0

    private let size: (width: UInt32, height: UInt32)
    private let context = CIContext()
    private let queue = DispatchQueue(label: "screen-share")
    private var timer: Timer?
    private var starting = false
    private var lastDemand = Date.distantPast
    private var refusedAt = Date.distantPast
    /// Touched only on `queue`.
    private var next = 0.0
    private var busy = false

    init() {
        let bounds = UIScreen.main.bounds
        let width = (Self.height * bounds.width / max(bounds.height, 1) / 2).rounded() * 2
        size = (UInt32(min(max(width, 200), Self.height)), UInt32(Self.height))
    }

    /// Ask iOS for screen sharing now, before anything needs the camera: its prompt
    /// interrupts the camera. `done` hears whether it was allowed. Once allowed the
    /// capture keeps running and pictures are sent only while Home watches, because
    /// starting it again later can bring the prompt back.
    func askFirst(_ done: @escaping (Bool) -> Void) {
        let recorder = RPScreenRecorder.shared()
        guard recorder.isAvailable else { done(false); return }
        // Already allowed this launch (the intro asked): do not ask again.
        if running { done(true); return }
        starting = true
        recorder.startCapture(handler: { [weak self] buffer, type, error in
            guard let self, error == nil, type == .video else { return }
            self.queue.async { if self.watched { self.send(buffer) } }
        }, completionHandler: { [weak self] error in
            DispatchQueue.main.async {
                self?.starting = false
                self?.running = error == nil
                done(error == nil)
            }
        })
    }
    /// The capture asked for at launch is running.
    private var running = false
    /// Touched only on `queue`: Home is watching, so pictures are sent.
    private var watched = false

    func start() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in self?.poll() }
    }

    private func poll() {
        let now = Date()
        // iOS ends a capture on its own, for example when the app leaves the screen.
        if sharing && !RPScreenRecorder.shared().isRecording { sharing = false }
        if spatial_mobile_screen_demand(size.width, size.height) != 0 { lastDemand = now }
        let wanted = now.timeIntervalSince(lastDemand) < Self.linger
        if running {
            // iOS ends a capture on its own, for example when the app leaves the screen.
            if !RPScreenRecorder.shared().isRecording { running = false; sharing = false; return }
            if sharing != wanted { sharing = wanted }
            queue.async { self.watched = wanted }
            return
        }
        if wanted && !sharing && !starting && now.timeIntervalSince(refusedAt) > Self.retryAfter {
            begin()
        } else if !wanted && sharing {
            end()
        }
    }

    private func begin() {
        let recorder = RPScreenRecorder.shared()
        guard recorder.isAvailable else { refusedAt = Date(); return }
        starting = true
        recorder.startCapture(handler: { [weak self] buffer, type, error in
            guard let self, error == nil, type == .video else { return }
            self.queue.async { self.send(buffer) }
        }, completionHandler: { [weak self] error in
            DispatchQueue.main.async {
                guard let self else { return }
                self.starting = false
                if error != nil { self.refusedAt = Date() } else { self.sharing = true }
            }
        })
    }

    private func end() {
        sharing = false
        RPScreenRecorder.shared().stopCapture { _ in }
    }

    /// On `queue`: scale to the size offered, encode and hand to the bridge.
    private func send(_ buffer: CMSampleBuffer) {
        let time = CMSampleBufferGetPresentationTimeStamp(buffer).seconds
        guard !busy, time.isFinite, time > 0, time >= next,
              let pixels = CMSampleBufferGetImageBuffer(buffer) else { return }
        busy = true
        defer { busy = false }
        next = time + Self.interval
        let image = CIImage(cvPixelBuffer: pixels)
        let width = Double(size.width), height = Double(size.height)
        let scaled = image
            .transformed(by: CGAffineTransform(scaleX: width / image.extent.width, y: height / image.extent.height))
            .cropped(to: CGRect(x: 0, y: 0, width: width, height: height))
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let jpeg = context.jpegRepresentation(of: scaled, colorSpace: space,
                  options: [kCGImageDestinationLossyCompressionQuality as CIImageRepresentationOption: 0.6])
        else { return }
        let timestamp = UInt64(time * 1_000_000_000)
        jpeg.withUnsafeBytes { bytes in
            if let base = bytes.bindMemory(to: UInt8.self).baseAddress {
                spatial_mobile_screen(timestamp, base, jpeg.count)
            }
        }
    }
}
