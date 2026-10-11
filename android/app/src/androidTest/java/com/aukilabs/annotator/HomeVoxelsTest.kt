package com.aukilabs.annotator

import android.opengl.Matrix
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class HomeVoxelsTest {
    private fun scene(count: Int = 1): JSONObject {
        val voxels = JSONArray()
        repeat(count) { voxels.put(JSONObject().put("center",JSONArray("[1,2,3]"))) }
        return JSONObject().put("frame","home").put("voxel_size",.1).put("voxels",voxels)
    }
    @Test fun cubesUseHomeVoxelSizeAndStayBounded() {
        val mesh = homeVoxelMesh(scene())!!
        assertEquals(1,mesh.count); assertEquals(72,mesh.vertices.remaining())
        val values = FloatArray(72); mesh.vertices.get(values)
        for (axis in 0..2) {
            val coordinates = values.filterIndexed { index,_ -> index%3 == axis }
            assertEquals(axis+1-.05f,coordinates.minOrNull()!!,.0001f)
            assertEquals(axis+1+.05f,coordinates.maxOrNull()!!,.0001f)
        }
        val bounded = homeVoxelMesh(scene(MAX_HOME_VOXELS+1))!!
        assertEquals(MAX_HOME_VOXELS,bounded.count)
        assertEquals(MAX_HOME_VOXELS+1,bounded.total)
        assertNull(homeVoxelMesh(scene().put("voxel_size",0)))
        assertEquals(0,homeVoxelMesh(scene(0))!!.vertices.remaining())
    }
    @Test fun inverseAlignmentIsAppliedExactlyOnceAndWrongScenesAreHidden() {
        val alignment = JSONObject("""{"session_to_home":{"to_frame_id":"home","translation":[4,1,3],"rotation_wxyz":[1,0,0,0]}}""")
        val vp = FloatArray(16).apply { Matrix.setIdentityM(this,0) }
        val mvp = FloatArray(16); val model = FloatArray(16)
        assertFalse(homeVoxelMatrix("home",null,vp,mvp,model))
        assertFalse(homeVoxelMatrix("other",alignment,vp,mvp,model))
        assertTrue(homeVoxelMatrix("home",alignment,vp,mvp,model))
        val point = FloatArray(4)
        Matrix.multiplyMV(point,0,mvp,0,floatArrayOf(4f,1f,1f,1f),0)
        assertArrayEquals(floatArrayOf(0f,0f,-2f,1f),point,.0001f)
    }
}
