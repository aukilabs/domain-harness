package com.aukilabs.annotator

import android.content.Context
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.os.SystemClock
import com.google.ar.core.Frame
import com.google.ar.core.Session

/** ARCore's timestamp epoch is unspecified: never equate it with nanoTime.
 * Frame.getAndroidCameraTimestamp and CameraConfig.getCameraId are present in
 * the pinned ARCore 1.48.0 API jar. The Frame API declares both timestamps for
 * the same captured image. Camera2 declares REALTIME as elapsedRealtimeNanos:
 * https://developer.android.com/reference/android/hardware/camera2/CameraCharacteristics#SENSOR_INFO_TIMESTAMP_SOURCE
 * Unsupported sources are not assigned a fabricated UTC relationship.
 */
internal class CaptureClock(context: Context, private val session: Session) {
    private val manager = context.getSystemService(CameraManager::class.java)
    private var checkedCamera: String? = null
    private var realtime = false
    var failure: String? = null
        private set

    fun observe(frame: Frame): Boolean {
        return try {
            val id = session.cameraConfig.cameraId
            if (checkedCamera != id) {
                realtime = manager.getCameraCharacteristics(id).get(CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE) ==
                    CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE_REALTIME
                checkedCamera = id
            }
            if (!realtime) {
                failure = "Camera clock has no supported system-time relationship"
                return false
            }
            if (frame.timestamp <= 0 || frame.androidCameraTimestamp <= 0) return false
            val generation = NativeBridge.clockGeneration()
            if (generation <= 0) return false
            val before = NativeBridge.sessionNow(generation)
            if (before <= 0) return false
            val now = SystemClock.elapsedRealtimeNanos()
            val after = NativeBridge.sessionNow(generation)
            // Both frame values are for the same exposure; capture delay cancels.
            val nativeNow = Math.addExact(frame.timestamp, Math.subtractExact(now, frame.androidCameraTimestamp))
            NativeBridge.clockSample(generation, 0, before, nativeNow, after).also {
                failure = if (it) null else "Waiting for a stable capture-clock measurement"
            }
        } catch (error: Exception) {
            failure = "Capture clock unavailable: ${error.javaClass.simpleName}"
            false
        }
    }
}
