package com.aukilabs.annotator

import org.junit.Assert.*
import org.junit.Test

class CalibrationStateTest {
    @Test fun acquisitionIsTenHzAndRechecksAreOneHzEvenAfterMisses() {
        val state = CalibrationState(); state.updateReference("scene")
        val first = 5_000_000_000L
        assertTrue(state.canDetect(first))
        state.attempted(first)
        assertFalse(state.canDetect(first+99_999_999L))
        assertTrue(state.canDetect(first+100_000_000L))
        assertTrue(state.accept(state.generation,first,first+10_000_000L))
        assertFalse(state.canDetect(first+1_000_000_000L))
        val recheck = first+1_010_000_000L
        assertTrue(state.canDetect(recheck))
        state.attempted(recheck) // A missed recheck must not return to 10 Hz.
        assertFalse(state.canDetect(recheck+100_000_000L))
        assertTrue(state.canDetect(recheck+1_000_000_000L))
        state.reset()
        assertTrue(state.canDetect(recheck+100_000_000L))
    }
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
