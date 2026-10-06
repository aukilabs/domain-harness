package com.aukilabs.annotator
import org.junit.Assert.*
import org.junit.Test
class LocalVoxelsTest {
    @Test fun depthFreshnessUsesMeasurementTimeNotRenderTime() {
        val fresh = DepthFreshness()
        assertFalse(fresh.accept(0))
        assertTrue(fresh.accept(100)) // Valid even if the rendering frame is at 120.
        assertFalse(fresh.accept(100))
        assertFalse(fresh.accept(99))
        assertTrue(fresh.accept(150))
        fresh.reset()
        assertTrue(fresh.accept(100))
    }
    @Test fun quantizesNegativeCoordinatesAndDeduplicates() {
        val map = LocalVoxels(); map.add(floatArrayOf(-.01f,.04f,-1.01f)); map.add(floatArrayOf(-.02f,.05f,-1.02f))
        assertEquals(setOf(Cell(-1,0,-11)),map.cells)
        map.clear(); assertTrue(map.cells.isEmpty())
    }
    @Test fun rejectsNonFiniteAndBoundsMemory() {
        val map = LocalVoxels(); map.add(floatArrayOf(Float.NaN,0f,0f)); assertTrue(map.cells.isEmpty())
        for (x in 0..100) for (y in 0..100) map.add(floatArrayOf(x.toFloat(),y.toFloat(),0f))
        assertEquals(10000,map.cells.size)
    }
}
