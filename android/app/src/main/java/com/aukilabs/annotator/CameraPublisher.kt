package com.aukilabs.annotator

import android.graphics.ImageFormat
import android.graphics.Rect
import android.graphics.YuvImage
import android.os.Process
import android.util.Log
import com.google.ar.core.Frame
import com.google.ar.core.exceptions.NotYetAvailableException
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executors

/** CPU-image copies only on GL; conversion/JPEG/JNI on a bounded background worker. */
internal class CameraPublisher {
    private val gate = ScanWorkGate()
    private val worker = Executors.newSingleThreadExecutor { Thread(it,"annotator-camera") }
    private var lastCapture = 0L
    private val planes = Array(3) { ByteArray(0) }
    private val nv21 = ByteArray(640*480*3/2)
    private val encoded = ByteArrayOutputStream(128*1024)

    fun capture(frame: Frame) {
        if (frame.timestamp-lastCapture < 100_000_000L) return
        lastCapture = frame.timestamp
        val token = gate.acquire() ?: return
        var demand = false
        gate.commit(token) { demand = NativeBridge.cameraDemand(true) }
        if (!demand) { gate.finish(); return }
        val captured = System.nanoTime()
        try {
            frame.acquireCameraImage().use { image ->
                require(image.width <= 1920 && image.height <= 1920)
                val strides = image.planes.map { it.rowStride to it.pixelStride }
                image.planes.forEachIndexed { i, p ->
                    val buffer = p.buffer.duplicate()
                    require(buffer.remaining() <= 1920*1920)
                    if (planes[i].size != buffer.remaining()) planes[i] = ByteArray(buffer.remaining())
                    buffer.get(planes[i])
                }
                val width = image.width; val height = image.height; val timestamp = frame.timestamp
                worker.execute {
                    try {
                        Process.setThreadPriority(Process.THREAD_PRIORITY_BACKGROUND)
                        cameraNv21(planes,strides,width,height,nv21)
                        encoded.reset()
                        if (YuvImage(nv21,ImageFormat.NV21,640,480,null)
                            .compressToJpeg(Rect(0,0,640,480),75,encoded)) {
                            val jpeg = encoded.toByteArray()
                            gate.commit(token) {
                                if (System.nanoTime()-captured < 500_000_000L) NativeBridge.camera(timestamp,jpeg)
                            }
                        }
                    } catch (e: Exception) {
                        Log.w("AnnotatorCamera","Camera encoding failed: ${e.javaClass.simpleName}")
                    } finally { gate.finish() }
                }
            }
        } catch (_: NotYetAvailableException) { gate.finish() }
        catch (e: Exception) {
            gate.finish()
            Log.w("AnnotatorCamera","Camera capture failed: ${e.javaClass.simpleName}")
        }
    }
    fun close() { gate.close { NativeBridge.cameraDemand(false) }; worker.shutdown() }
}

/** Center-crop to 4:3; keep sensor orientation, preserve aspect ratio and NV21 VU order. */
internal fun cameraNv21(planes: Array<ByteArray>, strides: List<Pair<Int,Int>>, width: Int, height: Int, target: ByteArray) {
    val unit = minOf(width/8,height/6)
    val cropWidth = unit*8
    val cropHeight = unit*6
    require(cropWidth >= 2 && cropHeight >= 2 && target.size == 640*480*3/2)
    val left = (width-cropWidth)/4*2; val top = (height-cropHeight)/4*2
    for (y in 0 until 480) for (x in 0 until 640) {
        val sx = left+x*cropWidth/640; val sy = top+y*cropHeight/480
        target[y*640+x] = planes[0][sy*strides[0].first+sx*strides[0].second]
    }
    for (y in 0 until 240) for (x in 0 until 320) {
        val sx = left/2+x*(cropWidth/2)/320; val sy = top/2+y*(cropHeight/2)/240
        val dst = 640*480+y*640+x*2
        target[dst] = planes[2][sy*strides[2].first+sx*strides[2].second]
        target[dst+1] = planes[1][sy*strides[1].first+sx*strides[1].second]
    }
}
