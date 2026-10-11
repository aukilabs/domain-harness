package com.aukilabs.annotator

import android.opengl.Matrix
import com.google.ar.core.Pose
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class AnnotationProjectionTest {
    @Test fun everyProjectionUsesTheCurrentCameraAndClearsOnAlignmentLoss() {
        val picker = AnnotationPicker(1f)
        picker.notes = """[{"id":"test","text":"Note","position":[0,0,-2],"pending":true}]"""
        val alignment = JSONObject("""{"session_to_home":{"translation":[0,0,0],"rotation_wxyz":[1,0,0,0]}}""")
        val vp = FloatArray(16)
        Matrix.perspectiveM(vp,0,90f,640f/480f,.1f,100f)
        val first = picker.project(alignment,vp,640,480).single()
        Matrix.translateM(vp,0,-.1f,0f,0f)
        // No waiting for a timer/UI frame: the very next call must move the marker.
        val next = picker.project(alignment,vp,640,480).single()
        assertEquals(320f,first.x,.001f); assertEquals(308f,next.x,.001f)
        assertTrue(next.pending)
        assertTrue(picker.project(null,vp,640,480).isEmpty())
        picker.notes = "[]"
        assertTrue(picker.project(alignment,vp,640,480).isEmpty())
    }
    @Test fun homeNoteProjectsThroughInverseCalibrationExactlyOnce() {
        val q = kotlin.math.sqrt(.5f)
        val home = Pose(floatArrayOf(4f,1f,3f),floatArrayOf(0f,q,0f,q))
        val projection = FloatArray(16)
        Matrix.perspectiveM(projection,0,90f,640f/480f,.1f,100f)
        fun note(point: FloatArray) = JSONObject().put("id","test").put("text","Note")
            .put("position",JSONArray(home.transformPoint(point).map { it.toDouble() }))
        val marker = projectNote(note(floatArrayOf(0f,0f,-2f)),home.inverse(),projection,640,480)!!
        assertEquals(320f,marker.x,.001f); assertEquals(240f,marker.y,.001f)
        assertNull(projectNote(note(floatArrayOf(0f,0f,2f)),home.inverse(),projection,640,480))
        assertNull(projectNote(note(floatArrayOf(100f,0f,-2f)),home.inverse(),projection,640,480))
    }
}
