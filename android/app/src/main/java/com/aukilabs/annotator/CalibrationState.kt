package com.aukilabs.annotator

/** GL-thread-owned epoch. Late detection results cannot restore a reset alignment. */
internal class CalibrationState {
    var generation = 0L; private set
    var reference: String? = null; private set
    var acceptedAt = 0L; private set
    fun updateReference(value: String?): Boolean {
        if (reference == value) return false
        reference = value; reset(); return true
    }
    fun reset() { generation++; acceptedAt = 0L }
    fun accept(epoch: Long, capturedAt: Long, now: Long): Boolean {
        if (epoch != generation || reference == null || capturedAt <= 0 || now < capturedAt || now-capturedAt > 2_000_000_000L) return false
        acceptedAt = now; return true
    }
    fun canDetect(now: Long) = reference != null && (acceptedAt == 0L || now-acceptedAt >= 1_000_000_000L)
}
