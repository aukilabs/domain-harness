package com.aukilabs.annotator

/** Pose cadence is independent of mapping; missed deadlines never create bursts. */
internal class PoseCadence {
    private var next = 0L
    fun due(timestamp: Long) = timestamp > 0 && timestamp >= next
    fun sent(timestamp: Long) {
        next = if (next == 0L || timestamp-next >= 200_000_000L) timestamp+200_000_000L
               else next+200_000_000L
    }
    fun reset() { next = 0L }
}
