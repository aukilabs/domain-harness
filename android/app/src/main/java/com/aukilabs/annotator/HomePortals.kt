package com.aukilabs.annotator

import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import kotlin.math.abs

internal const val MAX_HOME_PORTALS = 256
internal data class HomePortalMesh(val frame: String, val vertices: FloatBuffer, val count: Int)

/** Saved Home anchors, not the latest camera detections. Local portal axes are XY,
 * with +Y toward the top of the encoded square and +Z out of its printed face. */
internal fun homePortalMesh(reference: String?): HomePortalMesh? {
    if (reference == null) return null
    return runCatching {
        val scene = JSONObject(reference)
        val frame = scene.getJSONObject("map").getJSONObject("frame").getString("id")
        if (frame.isBlank()) return null
        val anchors = scene.getJSONObject("anchors")
        // Four boundary edges, center cross, top chevron and face-normal stem.
        val vertices = ByteBuffer.allocateDirect(minOf(anchors.length(),MAX_HOME_PORTALS)*18*3*4)
            .order(ByteOrder.nativeOrder()).asFloatBuffer()
        val keys = anchors.keys()
        var inspected = 0
        while (keys.hasNext() && inspected++ < MAX_HOME_PORTALS) {
            val anchor = anchors.optJSONObject(keys.next()) ?: continue
            val size = anchor.optDouble("side_length_m",Double.NaN)
            val transform = anchor.optJSONObject("pose_in_map") ?: continue
            if (!size.isFinite() || size <= 0 || size > 10 || transform.optString("to_frame_id") != frame) continue
            val t = transform.optJSONArray("translation") ?: continue
            val q = transform.optJSONArray("rotation_wxyz") ?: continue
            if (t.length() != 3 || q.length() != 4) continue
            if ((0..2).any { !t.optDouble(it).isFinite() || abs(t.optDouble(it)) > 1_000_000 }) continue
            val norm = (0..3).sumOf { q.optDouble(it)*q.optDouble(it) }
            if (!norm.isFinite() || abs(norm-1.0) > .001) continue
            val pose = poseFromJson(transform)
            val h = (size/2).toFloat()
            val local = floatArrayOf(
                -1f,-1f,0f, 1f,-1f,0f, 1f,-1f,0f, 1f,1f,0f,
                1f,1f,0f, -1f,1f,0f, -1f,1f,0f, -1f,-1f,0f,
                -.2f,0f,0f, .2f,0f,0f, 0f,-.2f,0f, 0f,.2f,0f,
                -.2f,.6f,0f, 0f,.8f,0f, 0f,.8f,0f, .2f,.6f,0f,
                0f,0f,0f, 0f,0f,.5f)
            for (i in local.indices step 3) {
                vertices.put(pose.transformPoint(floatArrayOf(local[i]*h,local[i+1]*h,local[i+2]*h)))
            }
        }
        vertices.flip()
        HomePortalMesh(frame,vertices,vertices.remaining()/3)
    }.getOrNull()
}
