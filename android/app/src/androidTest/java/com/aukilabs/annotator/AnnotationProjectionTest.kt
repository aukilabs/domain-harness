package com.aukilabs.annotator

import android.opengl.Matrix
import com.google.ar.core.Pose
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class AnnotationProjectionTest {
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
