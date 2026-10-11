package com.aukilabs.annotator

import org.junit.Assert.*
import org.junit.Test

class CalibrationStateTest {
    @Test fun resetsRejectInFlightDetections() {
        val state = CalibrationState()
        assertFalse(state.canDetect(1))
        state.updateReference("scene-a")
        val old = state.generation
        state.reset() // tracking lost / session reset
        assertFalse(state.accept(old,100,200))
        assertTrue(state.accept(state.generation,100,200))
        assertFalse(state.canDetect(300))
        assertTrue(state.canDetect(1_000_000_200))
        assertFalse(state.updateReference("scene-a")) // geometry-only updates keep alignment
        state.updateReference("scene-b")
        assertFalse(state.accept(old,100,200))
        state.updateReference(null)
        assertFalse(state.accept(state.generation,100,200))
    }
    @Test fun staleOrFutureResultsAreRejected() {
        val state = CalibrationState(); state.updateReference("scene")
        assertFalse(state.accept(state.generation,100,99))
        assertFalse(state.accept(state.generation,100,2_000_000_101))
        assertFalse(state.accept(state.generation,0,1))
        assertTrue(state.accept(state.generation,100,101))
    }
}
