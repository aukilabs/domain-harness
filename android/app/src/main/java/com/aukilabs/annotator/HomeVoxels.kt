package com.aukilabs.annotator

import android.opengl.Matrix
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer

internal const val MAX_HOME_VOXELS = 20_000
internal data class HomeVoxelMesh(val frame: String, val vertices: FloatBuffer, val count: Int, val total: Int)

/** Build a bounded wireframe buffer off the UI/GL threads; never modify the shared map. */
internal fun homeVoxelMesh(render: JSONObject, checkCancelled: () -> Unit = {}): HomeVoxelMesh? {
    val frame = render.optString("frame")
    val size = render.optDouble("voxel_size",Double.NaN)
    val voxels = render.optJSONArray("voxels") ?: return null
    if (frame.isBlank() || !size.isFinite() || size <= 0 || size > 10) return null
    val count = minOf(voxels.length(),MAX_HOME_VOXELS)
    val vertices = ByteBuffer.allocateDirect(count*24*3*4).order(ByteOrder.nativeOrder()).asFloatBuffer()
    val half = (size/2).toFloat()
    for (i in 0 until count) {
        if (i%256 == 0) checkCancelled()
        val index = (i.toLong()*voxels.length()/count).toInt()
        val center = voxels.getJSONObject(index).getJSONArray("center")
        if (center.length() != 3) return null
        val p = FloatArray(3) { center.getDouble(it).toFloat() }
        if (p.any { !it.isFinite() || kotlin.math.abs(it) > 1_000_000f }) return null
        // Twelve cube edges. Corners use three sign bits, one bit per axis.
        for (corner in 0..7) for (axis in 0..2) {
            if (corner and (1 shl axis) != 0) continue
            for (end in intArrayOf(corner,corner or (1 shl axis))) {
                for (coordinate in 0..2) vertices.put(p[coordinate]+if (end and (1 shl coordinate) == 0) -half else half)
            }
        }
    }
    vertices.flip()
    return HomeVoxelMesh(frame,vertices,count,voxels.length())
}

/** Home coordinates -> current AR session -> view/projection. Reject other scenes. */
internal fun homeVoxelMatrix(frame: String, alignment: JSONObject?, vp: FloatArray, target: FloatArray, model: FloatArray): Boolean {
    val transform = alignment?.optJSONObject("session_to_home") ?: return false
    if (transform.optString("to_frame_id") != frame) return false
    poseFromJson(transform).inverse().toMatrix(model,0)
    Matrix.multiplyMM(target,0,vp,0,model,0)
    return true
}
