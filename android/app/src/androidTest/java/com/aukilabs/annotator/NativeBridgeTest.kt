package com.aukilabs.annotator

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class NativeBridgeTest {
    @Test fun cameraBridgeIsSafeWithoutAConnectedPeer() {
        NativeBridge.cameraDemand(false)
        NativeBridge.camera(1,byteArrayOf(0))
    }
    @Test fun calibrationBridgeRunsSdkDetectorOnArm64() {
        val session = "12345678-1234-4234-8234-123456789abc"
        val frame = "local-scene:test-room:portal-ABCDEFGHIJK"
        val capture = """{
          "session":"$session","timestamp_ns":100,"width":64,"height":64,
          "fx":50,"fy":50,"cx":32,"cy":32,
          "camera_to_session":{"from_frame_id":"phone-camera:$session","to_frame_id":"phone-ar:$session","translation":[0,0,0],"rotation_wxyz":[1,0,0,0]},
          "scenegraph":{"map":{"map_id":"test","name":null,"domain_reference":null,
            "frame":{"id":"$frame","convention":"opengl","handedness":"right","up_axis":"Y","meters_per_unit":1,"origin_description":"test only"}},
            "anchors":{"ABCDEFGHIJK":{"anchor_id":"ABCDEFGHIJK","payload":"https://r8.hr/ABCDEFGHIJK","side_length_m":0.2,
              "pose_in_map":{"from_frame_id":"portal-test","to_frame_id":"$frame","translation":[0,0,0],"rotation_wxyz":[1,0,0,0]}}}}
        }"""
        val reply = NativeBridge.calibrate(ByteArray(64*64) { 255.toByte() },capture.toByteArray())
        assertNotNull(reply)
        val result = JSONObject(reply!!.toString(Charsets.UTF_8))
        assertTrue(result.isNull("alignment"))
        assertEquals("Show a Portal from the selected Home scene",result.getString("detail"))
        val bad = NativeBridge.calibrate(ByteArray(1),capture.toByteArray())!!
        assertTrue(JSONObject(bad.toString(Charsets.UTF_8)).getString("detail").contains("Invalid packed luma"))
        NativeBridge.depth(null); NativeBridge.pose(null)
    }
    @Test fun malformedProfileCompletesWithoutNetworkingOrLeakingCredentials() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val password = "not-a-real-password-測試"
        assertTrue(NativeBridge.start("{}".toByteArray(), "invalid@example.invalid".toByteArray(),
            password.toByteArray(), File(context.noBackupFilesDir, "test.identity").absolutePath.toByteArray()))
        try {
            val deadline = System.nanoTime() + 5_000_000_000L
            while (System.nanoTime() < deadline) {
                val bytes = NativeBridge.poll()
                if (bytes != null) {
                    val text = bytes.toString(Charsets.UTF_8)
                    assertFalse(text.contains(password))
                    val reply = JSONObject(text)
                    if (reply.optBoolean("finished")) {
                        assertEquals("error", reply.getJSONObject("update").getString("state"))
                        return
                    }
                }
                Thread.sleep(25)
            }
            fail("Rust client did not complete its validation failure")
        } finally { NativeBridge.stop() }
    }
}
