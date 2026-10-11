package com.aukilabs.annotator

import android.opengl.Matrix
import com.google.ar.core.*
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicReference

internal class AnnotationPicker(private val density: Float) {
    private data class Tap(val x: Float,val y: Float,val double: Boolean,val captured: Long)
    private val tap = AtomicReference<Tap?>(null)
    @Volatile var notes = "[]"
    private var parsedSource = ""
    private var parsed = emptyList<JSONObject>()
    fun tap(x: Float,y: Float,double: Boolean) { tap.compareAndSet(null,Tap(x,y,double,System.nanoTime())) }
    fun paused(pick: (AnnotationPick) -> Unit) {
        tap.getAndSet(null)?.let { pick(AnnotationPick("",it.double,error="Tracking paused; move slowly and try again")) }
    }
    fun frame(frame: Frame, alignment: JSONObject?, vp: FloatArray, width: Int, height: Int,
              pick: (AnnotationPick) -> Unit): List<ScreenNote> {
        val gesture = tap.getAndSet(null)
        val transform = alignment?.optJSONObject("session_to_home")
        if (transform == null) {
            if (gesture != null) pick(AnnotationPick("",gesture.double,error="Scan this scene's portal before annotating"))
            return emptyList()
        }
        val home = poseFromJson(transform)
        val markers = project(alignment,vp,width,height)
        if (gesture == null || System.nanoTime()-gesture.captured > 500_000_000L) return markers
        val frameId = transform.getString("to_frame_id")
        val existing = markers.filter { gesture.x in (it.x-28*density)..(it.x+(14+minOf(40,it.note.optString("text").length)*8)*density) &&
            gesture.y in (it.y-28*density)..(it.y+28*density) }
            .minByOrNull { (it.x-gesture.x)*(it.x-gesture.x)+(it.y-gesture.y)*(it.y-gesture.y) }
        if (existing != null) { pick(AnnotationPick(frameId,gesture.double,note=existing.note)); return markers }
        if (!gesture.double) return markers
        val hit = frame.hitTest(gesture.x,gesture.y).firstOrNull {
            it.distance in .05f..40f && it.trackable.trackingState == TrackingState.TRACKING && when (val trackable = it.trackable) {
                is Plane -> trackable.isPoseInPolygon(it.hitPose)
                is DepthPoint -> true
                else -> false
            }
        }
        if (hit == null) { pick(AnnotationPick(frameId,true,error="No measured surface here yet. Move slowly and try again.")); return markers }
        pick(AnnotationPick(frameId,true,origin=home.transformPoint(frame.camera.pose.translation),position=home.transformPoint(hit.hitPose.translation)))
        return markers
    }

    /** GL-thread projection for this exact camera frame, also used for picking. */
    internal fun project(alignment: JSONObject?, vp: FloatArray, width: Int, height: Int): List<ScreenNote> {
        val transform = alignment?.optJSONObject("session_to_home") ?: return emptyList()
        val source = notes
        if (source != parsedSource) {
            parsedSource = source
            parsed = runCatching { val array = JSONArray(source); (0 until array.length()).map { array.getJSONObject(it) } }.getOrDefault(emptyList())
        }
        val inverse = poseFromJson(transform).inverse()
        return parsed.mapNotNull { projectNote(it,inverse,vp,width,height) }
    }
}

internal fun projectNote(note: JSONObject, homeToSession: Pose, vp: FloatArray, width: Int, height: Int): ScreenNote? {
    val p = note.optJSONArray("position") ?: return null
    if (p.length() != 3) return null
    val local = homeToSession.transformPoint(FloatArray(3) { p.getDouble(it).toFloat() })
    val clip = FloatArray(4)
    Matrix.multiplyMV(clip,0,vp,0,floatArrayOf(local[0],local[1],local[2],1f),0)
    if (clip.any { !it.isFinite() } || clip[3] <= 0f) return null
    val x = clip[0]/clip[3]; val y = clip[1]/clip[3]; val z = clip[2]/clip[3]
    if (x !in -1f..1f || y !in -1f..1f || z !in -1f..1f) return null
    return ScreenNote(note,(x+1)*width/2,(1-y)*height/2,note.optBoolean("pending"))
}
