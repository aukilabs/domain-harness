import CoreLocation

/// Core Location's latest fix for this phone, and how far off its compass says it is.
final class GeoLocator: NSObject, CLLocationManagerDelegate {
    enum Permission { case undecided, granted, refused }
    static let freshFor: TimeInterval = 10
    private let manager = CLLocationManager()
    private var latest: (fix: GeoFix, at: Date)?
    /// Degrees, or nil while the compass has no true north.
    private(set) var headingError: Double?
    var onPermission: (() -> Void)?

    override init() {
        super.init()
        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyBest
    }

    var permission: Permission {
        switch manager.authorizationStatus {
        case .authorizedWhenInUse, .authorizedAlways: return .granted
        case .notDetermined: return .undecided
        default: return .refused
        }
    }

    /// The latest fix, unless it has gone stale.
    var fix: GeoFix? {
        guard let latest, -latest.at.timeIntervalSinceNow <= Self.freshFor else { return nil }
        return latest.fix
    }

    func requestPermission() { manager.requestWhenInUseAuthorization() }

    func start(heading: Bool) {
        manager.startUpdatingLocation()
        if heading { manager.startUpdatingHeading() }
    }

    func stopHeading() {
        manager.stopUpdatingHeading()
        headingError = nil
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) { onPermission?() }

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        // Core Location replays a cached fix first; a stale or invalid one is no use here.
        guard let location = locations.last, location.horizontalAccuracy >= 0,
              -location.timestamp.timeIntervalSinceNow <= Self.freshFor else { return }
        latest = (GeoFix(latitude: location.coordinate.latitude, longitude: location.coordinate.longitude,
                         altitude: location.altitude, accuracy: location.horizontalAccuracy), location.timestamp)
    }

    func locationManager(_ manager: CLLocationManager, didUpdateHeading newHeading: CLHeading) {
        headingError = newHeading.trueHeading >= 0 && newHeading.headingAccuracy >= 0 ? newHeading.headingAccuracy : nil
    }

    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {}

    // Let iOS ask for the figure-of-eight when the compass needs it.
    func locationManagerShouldDisplayHeadingCalibration(_ manager: CLLocationManager) -> Bool { true }
}
