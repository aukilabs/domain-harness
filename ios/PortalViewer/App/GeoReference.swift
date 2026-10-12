import Foundation
import simd

/// A GPS reading: degrees, metres above sea level, and the radius it is good to.
struct GeoFix: Equatable {
    var latitude: Double
    var longitude: Double
    var altitude: Double
    var accuracy: Double
}

/// Where a gravity-aligned, Y-up frame sits on the Earth: the coordinate of its
/// origin, and the direction of true north across its floor as (X, Z).
struct GeoReference: Equatable {
    var origin: GeoFix
    var north: SIMD2<Float>

    private static let metresPerDegree = 6_378_137.0 * Double.pi / 180

    init(origin: GeoFix, north: SIMD2<Float>) {
        self.origin = origin
        self.north = north
    }

    /// A frame known to hold `fix` at `point`.
    init(fix: GeoFix, at point: SIMD3<Float>, north: SIMD2<Float>) {
        self.init(origin: fix, north: north)
        origin = coordinate(of: -point)
    }

    /// East is north turned a quarter clockwise, seen from above.
    private var east: SIMD2<Float> { SIMD2(-north.y, north.x) }

    /// The coordinate of a point in this frame. Flat-earth, good for a building.
    func coordinate(of point: SIMD3<Float>) -> GeoFix {
        let floor = SIMD2(point.x, point.z)
        let northward = Double(simd_dot(floor, north)), eastward = Double(simd_dot(floor, east))
        let shrink = max(cos(origin.latitude * .pi / 180), 1e-6)
        return GeoFix(latitude: origin.latitude + northward / Self.metresPerDegree,
                      longitude: origin.longitude + eastward / (Self.metresPerDegree * shrink),
                      altitude: origin.altitude + Double(point.y), accuracy: origin.accuracy)
    }

    /// Degrees clockwise from true north of a direction across the floor, 0 to 360.
    func bearing(of direction: SIMD2<Float>) -> Double {
        let degrees = Double(atan2(simd_dot(direction, east), simd_dot(direction, north))) * 180 / .pi
        return degrees < 0 ? degrees + 360 : degrees
    }

    /// The same place on Earth, for a frame whose pose in this one is `pose`.
    func expressed(in pose: simd_float4x4) -> GeoReference {
        let turned = pose.inverse * SIMD4(north.x, 0, north.y, 0)
        let flat = SIMD2(turned.x, turned.z)
        let at = pose.columns.3
        return GeoReference(origin: coordinate(of: SIMD3(at.x, at.y, at.z)),
                            north: simd_length(flat) > 1e-4 ? simd_normalize(flat) : north)
    }
}

/// Decides when the compass has settled enough to take north from it, and when
/// to stop waiting for a compass that never will.
struct HeadingGate {
    static let holdFor: TimeInterval = 2
    static let giveUpAfter: TimeInterval = 15
    static let worstError = 20.0
    enum Verdict: Equatable { case waiting, established, gaveUp }
    private var began: TimeInterval?
    private var goodSince: TimeInterval?

    /// `headingError` is the compass's own estimate in degrees, nil when it has no true north.
    mutating func step(at time: TimeInterval, tracking: Bool, headingError: Double?, hasFix: Bool) -> Verdict {
        let began = self.began ?? time
        self.began = began
        if tracking, hasFix, let headingError, headingError <= Self.worstError {
            let since = goodSince ?? time
            goodSince = since
            if time - since >= Self.holdFor { return .established }
        } else {
            goodSince = nil
        }
        return time - began >= Self.giveUpAfter ? .gaveUp : .waiting
    }
}
