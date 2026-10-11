package com.aukilabs.annotator

/** GL-thread-owned epoch. Late detection results cannot restore a reset alignment. */
internal class CalibrationState {
    var generation = 0L; private set
    var reference: String? = null; private set
    var acceptedAt = 0L; private set
    private var lastAttempt = 0L
    fun updateReference(value: String?): Boolean {
        if (reference == value) return false
        reference = value; reset(); return true
    }
    fun reset() { generation++; acceptedAt = 0L; lastAttempt = 0L }
    fun attempted(now: Long) { lastAttempt = now }
    fun accept(epoch: Long, capturedAt: Long, now: Long): Boolean {
        if (epoch != generation || reference == null || capturedAt <= 0 || now < capturedAt || now-capturedAt > 2_000_000_000L) return false
        acceptedAt = now; return true
    }
    fun canDetect(now: Long): Boolean {
        if (reference == null || now <= 0L) return false
        val interval = if (acceptedAt == 0L) 100_000_000L else 1_000_000_000L
        return (lastAttempt == 0L || now-lastAttempt >= interval) &&
            (acceptedAt == 0L || now-acceptedAt >= interval)
    }
}
