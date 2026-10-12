import Foundation
import simd

enum PortalPose {
    // Corners are printed top-left, top-right, bottom-right, bottom-left in
    // capture-image pixels (origin top-left). Portal axes: right, up, out.
    static func estimate(corners: [SIMD2<Float>], side: Float,
                         intrinsics k: simd_float3x3) -> simd_float4x4? {
        guard corners.count == 4, side > 0, side.isFinite else { return nil }
        let half = Double(side / 2)
        let object = [SIMD2(-half, half), SIMD2(half, half), SIMD2(half, -half), SIMD2(-half, -half)]
        let inverseK = k.inverse
        var equations = [[Double]]()
        for (p, pixel) in zip(object, corners) {
            let ray = inverseK * SIMD3(pixel.x, pixel.y, 1)
            let u = Double(ray.x / ray.z), v = Double(ray.y / ray.z)
            equations.append([p.x, p.y, 1, 0, 0, 0, -u*p.x, -u*p.y, u])
            equations.append([0, 0, 0, p.x, p.y, 1, -v*p.x, -v*p.y, v])
        }
        guard let h = solve(equations) else { return nil }
        let a = SIMD3<Float>(Float(h[0]), Float(h[3]), Float(h[6]))
        let b = SIMD3<Float>(Float(h[1]), Float(h[4]), Float(h[7]))
        let scale = 2 / (simd_length(a) + simd_length(b))
        let x = simd_normalize(a)
        let y = simd_normalize(b - simd_dot(b, x) * x)
        let z = simd_cross(x, y)
        let t = SIMD3<Float>(Float(h[2]), Float(h[5]), 1) * scale
        guard t.z > 0.1, t.z < 8, t.x.isFinite, t.y.isFinite,
              simd_dot(z, t) < 0 else { return nil }
        // Reject inconsistent corners and unstable planar solutions.
        var error: Float = 0
        for (p, pixel) in zip(object, corners) {
            let point = x * Float(p.x) + y * Float(p.y) + t
            guard point.z > 0 else { return nil }
            let projected = k * point
            error += simd_length_squared(SIMD2(projected.x, projected.y) / projected.z - pixel)
        }
        // Vision's corner noise grows with the code's size in the
        // image, so allow 2% of its mean edge length, never less than 3 px.
        var edge: Float = 0
        for i in 0..<4 { edge += simd_length(corners[i] - corners[(i + 1) % 4]) }
        let tolerance = max(3, 0.02 * edge / 4)
        guard error.isFinite, sqrt(error / 4) < tolerance else { return nil }
        let optical = simd_float4x4(SIMD4(x, 0), SIMD4(y, 0), SIMD4(z, 0), SIMD4(t, 1))
        // Camera optical +Y down/+Z forward -> ARKit +Y up/+Z backward.
        let conversion = simd_float4x4(diagonal: SIMD4<Float>(1, -1, -1, 1))
        return conversion * optical
    }

    private static func solve(_ input: [[Double]]) -> [Double]? {
        var a = input
        for col in 0..<8 {
            let pivot = (col..<8).max { abs(a[$0][col]) < abs(a[$1][col]) }!
            guard abs(a[pivot][col]) > 1e-10 else { return nil }
            a.swapAt(col, pivot)
            let divisor = a[col][col]
            for j in col...8 { a[col][j] /= divisor }
            for row in 0..<8 where row != col {
                let factor = a[row][col]
                for j in col...8 { a[row][j] -= factor * a[col][j] }
            }
        }
        let answer = a.map { $0[8] }
        return answer.allSatisfy(\.isFinite) ? answer : nil
    }
}

/// Fuses Portal sightings into one Home-to-AR alignment.
///
/// ARKit's world and Home's frame are both gravity-aligned with Y up, so the
/// alignment is only a turn about Y plus a translation. A Portal is treated as
/// something that may be tilted or held in the hand: its tilt never tips or
/// rolls the world, and the more its measured tilt disagrees with gravity, the
/// less its heading is trusted. Sightings are averaged by quality instead of
/// each one replacing the last.
struct AlignmentFilter {
    private(set) var yaw: Float = 0
    private(set) var translation = SIMD3<Float>(repeating: 0)
    private(set) var weight: Float = 0
    private(set) var sightings = 0
    private var outliers = 0

    /// Beyond this tilt from its stored pose a Portal is not where the scene says.
    static let handHeldTilt: Float = 35 * .pi / 180
    static let maxWeight: Float = 6

    var isLocked: Bool { weight > 0 }
    var transform: simd_float4x4 { Self.matrix(yaw: yaw, translation: translation) }

    mutating func reset() { self = AlignmentFilter() }

    static func matrix(yaw: Float, translation: SIMD3<Float>) -> simd_float4x4 {
        var m = simd_float4x4(simd_quatf(angle: yaw, axis: SIMD3(0, 1, 0)))
        m.columns.3 = SIMD4(translation, 1)
        return m
    }
    private static func wrap(_ angle: Float) -> Float { atan2(sin(angle), cos(angle)) }

    enum Outcome: Equatable { case locked, averaged, ignoredTilted, ignoredOutlier, relocked }

    /// - Parameters:
    ///   - measured: AR-from-Home implied by this one sighting, tilt included.
    ///   - portalInAR: where the Portal's center was measured in AR.
    ///   - portalInHome: where the scene says its center is.
    ///   - edgePixels: mean edge length of the code in the image.
    mutating func add(measured: simd_float4x4, portalInAR: SIMD3<Float>, portalInHome: SIMD3<Float>,
                      edgePixels: Float) -> Outcome {
        // Swing-twist about Y: the twist is the heading, the swing is how far
        // this sighting's idea of "up" is from gravity.
        let q = simd_quatf(measured)
        let measuredYaw = Self.wrap(2 * atan2(q.imag.y, q.real))
        let tilt = acos(max(-1, min(1, measured.columns.1.y)))
        let size = max(0.2, min(2, edgePixels / 150))
        let tiltTrust = exp(-pow(tilt / (12 * .pi / 180), 2))
        let w = size * size * max(0.02, tiltTrust)
        let handHeld = tilt > Self.handHeldTilt

        func pinned(_ heading: Float) -> SIMD3<Float> {
            // Keep the Portal's center where it was seen.
            let turned = simd_quatf(angle: heading, axis: SIMD3(0, 1, 0)).act(portalInHome)
            return portalInAR - turned
        }
        guard isLocked else {
            // Anything is better than nothing for a first lock; a tilted
            // Portal locks weakly so the next good sighting dominates.
            yaw = measuredYaw; translation = pinned(measuredYaw)
            weight = handHeld ? 0.05 : w; sightings = 1; outliers = 0
            return .locked
        }
        if handHeld { return .ignoredTilted }
        let headingError = Self.wrap(measuredYaw - yaw)
        let predicted = simd_quatf(angle: yaw, axis: SIMD3(0, 1, 0)).act(portalInHome) + translation
        if abs(headingError) > 20 * .pi / 180 || simd_distance(predicted, portalInAR) > 0.35 {
            // One wild read is noise. Three in a row means the world really is elsewhere.
            outliers += 1
            guard outliers >= 3 else { return .ignoredOutlier }
            yaw = measuredYaw; translation = pinned(measuredYaw)
            weight = w; sightings = 1; outliers = 0
            return .relocked
        }
        outliers = 0
        let gain = w / (weight + w)
        yaw = Self.wrap(yaw + gain * headingError)
        translation += gain * (pinned(yaw) - translation)
        weight = min(Self.maxWeight, weight + w)
        sightings += 1
        return .averaged
    }
}
