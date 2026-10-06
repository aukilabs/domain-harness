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
