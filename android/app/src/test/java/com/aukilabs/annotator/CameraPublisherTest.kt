package com.aukilabs.annotator

import org.junit.Assert.*
import org.junit.Test

class CameraPublisherTest {
    @Test fun paddedPixelStridedPlanesProduceNv21InVuOrder() {
        val y = ByteArray(10*6) { (it/10*8+it%10).toByte() }
        val planes = arrayOf(y,ByteArray(10*3) { 10 },ByteArray(10*3) { 20 })
        val out = ByteArray(640*480*3/2)
        cameraNv21(planes,listOf(10 to 1,10 to 2,10 to 2),8,6,out)
        assertEquals(0.toByte(),out[0])
        assertEquals(7.toByte(),out[639])
        assertEquals(40.toByte(),out[479*640])
        assertEquals(47.toByte(),out[480*640-1])
        assertEquals(20.toByte(),out[480*640])
        assertEquals(10.toByte(),out[480*640+1])
    }
    @Test fun wideImagesAreCenterCroppedNotStretched() {
        val luma = ByteArray(16*6) { (it%16).toByte() }
        val out = ByteArray(640*480*3/2)
        cameraNv21(arrayOf(luma,ByteArray(24),ByteArray(24)),listOf(16 to 1,8 to 1,8 to 1),16,6,out)
        assertEquals(4.toByte(),out[0])
        assertEquals(11.toByte(),out[639])
    }
}
