package com.aukilabs.annotator

import android.graphics.BitmapFactory
import android.graphics.ImageFormat
import android.graphics.Rect
import android.graphics.YuvImage
import java.io.ByteArrayOutputStream
import org.junit.Assert.*
import org.junit.Test

class CameraEncodingTest {
    @Test fun cameraYuvProducesDecodableContractSizedJpeg() {
        val nv21 = ByteArray(640*480*3/2)
        cameraNv21(arrayOf(ByteArray(640*480) { 100 },ByteArray(320*240) { 128.toByte() },
            ByteArray(320*240) { 128.toByte() }),listOf(640 to 1,320 to 1,320 to 1),640,480,nv21)
        val out = ByteArrayOutputStream()
        assertTrue(YuvImage(nv21,ImageFormat.NV21,640,480,null).compressToJpeg(Rect(0,0,640,480),75,out))
        val bytes = out.toByteArray()
        val bitmap = BitmapFactory.decodeByteArray(bytes,0,bytes.size)
        assertNotNull(bitmap)
        assertEquals(640,bitmap.width); assertEquals(480,bitmap.height)
        bitmap.recycle()
    }
}
