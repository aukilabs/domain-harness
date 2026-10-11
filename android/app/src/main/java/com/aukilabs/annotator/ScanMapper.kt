package com.aukilabs.annotator

import com.google.ar.core.Pose
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.ceil
import kotlin.math.sqrt

internal data class DepthCopy(
    val width: Int, val height: Int,
    val depth: ByteArray, val depthRow: Int, val depthPixel: Int,
    val confidence: ByteArray, val confidenceRow: Int, val confidencePixel: Int,
    val fx: Float, val fy: Float, val cx: Float, val cy: Float,
)
internal data class ScanInput(
    val session: String, val timestamp: Long, val pose: Pose,
    val alignment: JSONObject?, val depth: DepthCopy?, val depthStatus: String,
    val publish: Boolean,
)
internal data class ScanOutput(
    val centers: FloatArray, val count: Int, val depthStatus: String,
    val local: ByteArray?, val depth: ByteArray?,
)

/** Confined to one worker. No Frame, Image or GL objects cross the thread boundary. */
internal class ScanMapper {
    private val voxels = LocalVoxels()
    private var session = ""
    private var centers = FloatArray(0)
    private var encodedCells = "[]"
    private val point = FloatArray(3)
    private val transformed = FloatArray(3)

    fun process(input: ScanInput): ScanOutput {
        if (session != input.session) {
            session = input.session; voxels.clear(); centers = FloatArray(0); encodedCells = "[]"
        }
        val previousCount = voxels.cells.size
        var status = input.depthStatus
        var depthMessage: ByteArray? = null
        input.depth?.let { d ->
            val step = ceil(sqrt(d.width * d.height / 2048.0)).toInt().coerceAtLeast(1)
            var confident = 0; var hits = 0; var maximum = 0; var rawCount = 0
            val raw = if (input.alignment != null) StringBuilder(64 * 1024).append('[') else null
            for (y in 0 until d.height step step) for (x in 0 until d.width step step) {
                val certainty = d.confidence[y*d.confidenceRow+x*d.confidencePixel].toInt() and 255
                maximum = maxOf(maximum, certainty)
                if (certainty < 128) continue
                confident++
                val index = y*d.depthRow+x*d.depthPixel
                val z = ((d.depth[index].toInt() and 255) or ((d.depth[index+1].toInt() and 255) shl 8)) * .001f
                if (z < .2f || z > 5f) continue
                point[0] = (x-d.cx)*z/d.fx; point[1] = -(y-d.cy)*z/d.fy; point[2] = -z
                input.pose.transformPoint(point, 0, transformed, 0)
                voxels.add(transformed)
                if (raw != null && rawCount < 2048 && point.all { it.isFinite() } &&
                    point[0]*point[0]+point[1]*point[1]+point[2]*point[2] <= 64f) {
                    if (rawCount++ > 0) raw.append(',')
                    raw.append('[').append(point[0]).append(',').append(point[1]).append(',').append(point[2]).append(']')
                }
                hits++
            }
            status = "Depth ${d.width}×${d.height}: $hits hits · $confident confident · max confidence $maximum/255"
            if (raw != null && rawCount > 0 && input.timestamp >= input.alignment!!.getLong("timestamp_ns")) {
                raw.append(']')
                val metadata = JSONObject().put("session",session).put("timestamp_ns",input.timestamp)
                    .put("camera_to_session",poseJson(input.pose,"phone-camera:$session","phone-ar:$session"))
                    .put("alignment",input.alignment).toString()
                depthMessage = (metadata.dropLast(1) + ",\"points\":" + raw + "}").toByteArray()
            }
        }
        if (previousCount != voxels.cells.size) {
            centers = voxels.centers()
            encodedCells = voxels.cellsJson()
        }
        val local = if (input.publish)
            "{\"session\":${JSONObject.quote(session)},\"timestamp_ns\":${input.timestamp},\"cells\":$encodedCells}".toByteArray() else null
        return ScanOutput(centers, voxels.cells.size, status, local, depthMessage)
    }
}

internal fun encodePhonePose(session: String, timestamp: Long, pose: Pose, alignment: JSONObject): ByteArray {
    val transform = alignment.getJSONObject("session_to_home")
    return JSONObject().put("session",session).put("timestamp_ns",timestamp)
        .put("pose",poseJson(poseFromJson(transform).compose(pose),"phone-camera:$session",transform.getString("to_frame_id")))
        .toString().toByteArray()
}

internal fun poseJson(p: Pose, from: String, to: String): JSONObject {
    val q = p.rotationQuaternion
    return JSONObject().put("from_frame_id",from).put("to_frame_id",to)
        .put("translation",JSONArray(p.translation.map { it.toDouble() }))
        .put("rotation_wxyz",JSONArray(listOf(q[3].toDouble(),q[0].toDouble(),q[1].toDouble(),q[2].toDouble())))
}
internal fun poseFromJson(p: JSONObject): Pose {
    val t = p.getJSONArray("translation"); val q = p.getJSONArray("rotation_wxyz")
    return Pose(FloatArray(3) { t.getDouble(it).toFloat() },floatArrayOf(q.getDouble(1).toFloat(),q.getDouble(2).toFloat(),q.getDouble(3).toFloat(),q.getDouble(0).toFloat()))
}
