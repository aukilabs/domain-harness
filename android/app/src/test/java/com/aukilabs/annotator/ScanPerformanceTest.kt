package com.aukilabs.annotator

import java.nio.ByteBuffer
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.*
import org.junit.Test

class ScanPerformanceTest {
    @Test fun posePublishesFiveTimesPerSecondWithoutCatchupBursts() {
        val cadence = PoseCadence()
        var count = 0
        for (timestamp in 1L until 1_000_000_001L step 10_000_000L) {
            if (cadence.due(timestamp)) { count++; cadence.sent(timestamp) }
        }
        assertEquals(5,count)
        cadence.sent(5_000_000_000L)
        assertFalse(cadence.due(5_000_000_001L))
        assertTrue(cadence.due(5_200_000_000L))
        cadence.reset(); assertTrue(cadence.due(1))
    }
    @Test fun captureNeverWaitsForAnInFlightCommit() {
        val gate = ScanWorkGate()
        val token = gate.acquire()!!
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val worker = thread {
            gate.commit(token) { entered.countDown(); check(release.await(5,TimeUnit.SECONDS)) }
            gate.finish()
        }
        try {
            assertTrue(entered.await(5,TimeUnit.SECONDS))
            assertNull(gate.acquire()) // Returns while worker still holds commit lock.
        } finally { release.countDown(); worker.join(5000) }
        assertFalse(worker.isAlive)
    }
    @Test fun workerIsBoundedAndResetDoesNotReuseInFlightBuffers() {
        val gate = ScanWorkGate()
        val old = gate.acquire()!!
        repeat(100) { assertNull(gate.acquire()) }
        var value = "old"
        gate.invalidate { value = "reset" }
        assertNull(gate.acquire()) // Old job still owns the copied image buffers.
        assertFalse(gate.commit(old) { value = "stale" })
        assertEquals("reset",value)
        gate.finish()
        val fresh = gate.acquire()!!
        assertTrue(gate.commit(fresh) { value = "fresh" })
        gate.close { value = "closed" }
        assertFalse(gate.commit(fresh) { value = "late" })
        gate.finish()
        assertNull(gate.acquire())
        assertEquals("closed",value)
    }
    @Test fun lumaCopiesPackedAndStridedPlanesWithoutChangingSourcePosition() {
        val pixels = byteArrayOf(99,1,2,0,0,3,4)
        val source = ByteBuffer.wrap(pixels).apply { position(1) }
        val target = ByteArray(4)
        copyLuma(source,2,2,4,1,target)
        assertArrayEquals(byteArrayOf(1,2,3,4),target)
        assertEquals(1,source.position())
        copyLuma(ByteBuffer.wrap(byteArrayOf(1,0,2,0,3,0,4)),2,2,4,2,target)
        assertArrayEquals(byteArrayOf(1,2,3,4),target)
    }
    @Test(expected = IllegalArgumentException::class)
    fun lumaRejectsTruncatedPlane() {
        copyLuma(ByteBuffer.wrap(byteArrayOf(1,2,3)),2,2,2,1,ByteArray(4))
    }
    @Test fun directVoxelEncodingPreservesSignedCellsAndCenters() {
        val voxels = LocalVoxels()
        assertEquals("[]",voxels.cellsJson())
        voxels.add(floatArrayOf(-.01f,.04f,-1.01f))
        voxels.add(floatArrayOf(1.01f,2.01f,3.01f))
        assertEquals("[[-1,0,-11],[10,20,30]]",voxels.cellsJson())
        assertArrayEquals(floatArrayOf(-.05f,.05f,-1.05f,1.05f,2.05f,3.05f),voxels.centers(),.00001f)
    }
    @Test fun metricsDistinguishUniqueCameraFramesAndExcludeArUpdateFromWork() {
        val metrics = ScanMetrics()
        var report: String? = null
        for (i in 0..100) {
            val start = 1L+i*50_000_000L
            report = metrics.record(start,start+25_000_000,20_000_000,1L+i/2) ?: report
        }
        assertNotNull(report)
        assertTrue(report!!,report.contains("10 camera fps"))
        assertTrue(report,report.contains("20 draw fps"))
        assertTrue(report,report.contains("work 5 ms avg / 5 ms max"))
        assertTrue(report,report.contains("0 frames >50 ms"))
    }
}
