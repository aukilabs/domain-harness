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
