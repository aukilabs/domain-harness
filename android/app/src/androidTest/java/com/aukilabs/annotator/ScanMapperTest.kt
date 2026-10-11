package com.aukilabs.annotator

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.ar.core.Pose
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ScanMapperTest {
    @Test fun workerPreservesCapturePoseDepthConfidenceAndSessionIsolation() {
        val mapper = ScanMapper()
        val pose = Pose(floatArrayOf(1f,0f,0f),floatArrayOf(0f,0f,0f,1f))
        // Padded two-pixel planes: only the first pixel is confident.
        val depth = DepthCopy(2,1,byteArrayOf(0xe8.toByte(),3,0xe8.toByte(),3),4,2,
            byteArrayOf(255.toByte(),0),2,1,1f,1f,0f,0f)
        val aligned = JSONObject().put("timestamp_ns",1)
            .put("session_to_home",poseJson(Pose.IDENTITY,"phone-ar:a","home"))
        val result = mapper.process(ScanInput("a",100,pose,aligned,depth,"",true))
        assertEquals(1,result.count)
        assertEquals("[[10,0,-10]]",JSONObject(String(result.local!!)).getJSONArray("cells").toString())
        val raw = JSONObject(String(result.depth!!))
        assertEquals(100,raw.getLong("timestamp_ns"))
        assertEquals(-1.0,raw.getJSONArray("points").getJSONArray(0).getDouble(2),0.000001)
        assertEquals(1.0,raw.getJSONObject("camera_to_session").getJSONArray("translation").getDouble(0),0.0)
        val presence = JSONObject(String(encodePhonePose("a",100,pose,aligned)))
        assertEquals("home",presence.getJSONObject("pose").getString("to_frame_id"))
        assertEquals(1.0,presence.getJSONObject("pose").getJSONArray("translation").getDouble(0),0.0)
        val repeated = mapper.process(ScanInput("a",200,pose,null,null,"retained",true))
        assertSame(result.centers,repeated.centers) // No new buffer for an unchanged map.
        assertNull(repeated.depth)
        assertEquals(1,repeated.count)
        val reset = mapper.process(ScanInput("b",300,pose,null,null,"new session",true))
        assertEquals(0,reset.count)
        assertEquals(0,JSONObject(String(reset.local!!)).getJSONArray("cells").length())
    }

    @Test fun compareFullMapPreparationWithoutPublishingAnyObservations() {
        val voxels = LocalVoxels()
        for (x in 0 until 100) for (y in 0 until 100) voxels.add(floatArrayOf(x.toFloat(),y.toFloat(),0f))
        fun legacy(): Pair<FloatArray,String> {
            val centers = voxels.cells.flatMap { listOf((it.x+.5f)*.1f,(it.y+.5f)*.1f,(it.z+.5f)*.1f) }.toFloatArray()
            val json = JSONArray()
            for (c in voxels.cells) json.put(JSONArray(listOf(c.x,c.y,c.z)))
            return centers to json.toString()
        }
        val old = legacy()
        assertArrayEquals(old.first,voxels.centers(),0f)
        assertEquals(old.second,voxels.cellsJson())
        repeat(3) { legacy(); voxels.centers(); voxels.cellsJson() }
        val start = System.nanoTime()
        repeat(20) { legacy() }
        val middle = System.nanoTime()
        repeat(20) { voxels.centers(); voxels.cellsJson() }
        val end = System.nanoTime()
        // Diagnostic, not a flaky timing assertion; this is not end-to-end AR FPS.
        Log.i("AnnotatorPerf","10k-cell preparation, 20 iterations: legacy ${(middle-start)/1_000_000} ms; direct ${(end-middle)/1_000_000} ms")
    }
}
