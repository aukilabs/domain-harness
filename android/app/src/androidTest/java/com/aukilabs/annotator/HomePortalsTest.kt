package com.aukilabs.annotator

import android.opengl.Matrix
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class HomePortalsTest {
    private fun anchor(size: Double = .15, rotation: String = "[1,0,0,0]") = JSONObject("""
        {"side_length_m":$size,"pose_in_map":{"to_frame_id":"home",
        "translation":[4,1,3],"rotation_wxyz":$rotation}}
    """)
    private fun scene(anchors: JSONObject) = JSONObject().put("map",JSONObject("""{"frame":{"id":"home"}}"""))
        .put("anchors",anchors).toString()

    @Test fun squareUsesSavedSizeAndFullAnchorPose() {
        val h = kotlin.math.sqrt(.5)
        val mesh = homePortalMesh(scene(JSONObject().put("arbitrary-id",anchor(rotation="[$h,-$h,0,0]"))))!!
        assertEquals("home",mesh.frame); assertEquals(18,mesh.count)
        val values = FloatArray(mesh.vertices.remaining()); mesh.vertices.get(values)
        // Floor-mounted square: its local XY plane becomes Home XZ, with +Y normal.
        for (i in 0 until 8) assertEquals(1f,values[i*3+1],.00001f)
        assertArrayEquals(floatArrayOf(3.925f,1f,3.075f),values.take(3).toFloatArray(),.00001f)
        assertArrayEquals(floatArrayOf(4f,1.0375f,3f),values.takeLast(3).toFloatArray(),.00001f)
        val alignment = JSONObject("""{"session_to_home":{"to_frame_id":"home","translation":[4,1,3],"rotation_wxyz":[1,0,0,0]}}""")
        val identity = FloatArray(16).apply { Matrix.setIdentityM(this,0) }
        val mvp = FloatArray(16); val model = FloatArray(16)
        assertFalse(homeVoxelMatrix(mesh.frame,null,identity,mvp,model))
        assertFalse(homeVoxelMatrix("different-scene",alignment,identity,mvp,model))
        assertTrue(homeVoxelMatrix(mesh.frame,alignment,identity,mvp,model))
        val projected = FloatArray(4)
        Matrix.multiplyMV(projected,0,mvp,0,floatArrayOf(values[0],values[1],values[2],1f),0)
        assertArrayEquals(floatArrayOf(-.075f,0f,.075f,1f),projected,.00001f)
    }

    @Test fun multiplePortalsUseTheirOwnSizesAndMalformedAnchorsAreSkipped() {
        val anchors = JSONObject().put("one",anchor()).put("two",anchor(.3))
            .put("bad-size",anchor(-1.0)).put("bad-quaternion",anchor(rotation="[0,0,0,0]"))
            .put("wrong-frame",anchor().apply { getJSONObject("pose_in_map").put("to_frame_id","other") })
            .put("missing",JSONObject())
        val mesh = homePortalMesh(scene(anchors))!!
        assertEquals(36,mesh.count)
        val xs = (0 until mesh.count).map { mesh.vertices.get(it*3) }
        assertEquals(3.85f,xs.minOrNull()!!,.00001f)
        assertEquals(4.15f,xs.maxOrNull()!!,.00001f)
        assertNull(homePortalMesh(null)); assertNull(homePortalMesh("bad json"))
        assertEquals(0,homePortalMesh(scene(JSONObject()))!!.count)
    }

    @Test fun geometryIsBounded() {
        val anchors = JSONObject()
        repeat(MAX_HOME_PORTALS+10) { anchors.put("portal-$it",anchor()) }
        assertEquals(MAX_HOME_PORTALS*18,homePortalMesh(scene(anchors))!!.count)
    }
}
