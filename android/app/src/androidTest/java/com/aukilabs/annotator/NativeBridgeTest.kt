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
        val diagnostics = result.getJSONObject("diagnostics")
        assertEquals(0,diagnostics.getInt("decoded"))
        assertEquals(0,diagnostics.getInt("matched"))
        assertTrue(diagnostics.getLong("total_us") >= diagnostics.getLong("decode_us"))
        // Real QR pixels through JNI on the device, no peer or synthetic Home writes.
        val fullCapture = JSONObject(capture).put("width",640).put("height",480)
            .put("fx",500).put("fy",500).put("cx",320).put("cy",240).toString().toByteArray()
        val pixels = ByteArray(640*480) { 255.toByte() }
        qrFixture.lines().forEachIndexed { y, row -> row.forEachIndexed { x, bit ->
            if (bit == '1') for (dy in 0..5) for (dx in 0..5) pixels[(100+y*6+dy)*640+200+x*6+dx] = 0
        } }
        repeat(5) {
            val aligned = JSONObject(NativeBridge.calibrate(pixels,fullCapture)!!.toString(Charsets.UTF_8))
            assertEquals("ABCDEFGHIJK",aligned.getJSONObject("alignment").getString("portal_id"))
            assertEquals(1,aligned.getJSONObject("diagnostics").getInt("matched"))
            android.util.Log.i("AnnotatorCalibration","synthetic_fixture=$it diagnostics=${aligned.getJSONObject("diagnostics")}")
        }
        val bad = NativeBridge.calibrate(ByteArray(1),capture.toByteArray())!!
        assertTrue(JSONObject(bad.toString(Charsets.UTF_8)).getString("detail").contains("Invalid packed luma"))
        NativeBridge.depth(null); NativeBridge.pose(null)
    }
    // qrcode crate's standard encoding of https://r8.hr/ABCDEFGHIJK, 25x25 modules.
    private val qrFixture = """
        1111111001010111001111111
        1000001001111001001000001
        1011101000010011001011101
        1011101001001110101011101
        1011101001010001001011101
        1000001010010101101000001
        1111111010101010101111111
        0000000001110011100000000
        1001011010011011010100000
        1001010010111011001100001
        0001101100010111100100011
        1110000101101000100110000
        0010001001011110101101011
        0001100010111010011101101
        1010111011101000100110101
        0100110010101001011010010
        1110001101010000111111100
        0000000011100101100011001
        1111111000000101101011011
        1000001010010101100011100
        1011101000111010111111000
        1011101011011111100111110
        1011101000011000000110101
        1000001001011110101001000
        1111111010011011101100011
    """.trimIndent()
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
