package com.aukilabs.annotator

/** Render-thread metrics, not Compose UI timings. Reports bounded five-second windows. */
internal class ScanMetrics {
    private var started = 0L
    private var frames = 0
    private var cameraFrames = 0
    private var lastCamera = 0L
    private var workNs = 0L
    private var maxWorkNs = 0L
    private var slowFrames = 0

    fun record(start: Long, end: Long, updateNs: Long, cameraTimestamp: Long): String? {
        if (started == 0L) started = start
        frames++
        if (cameraTimestamp > 0 && cameraTimestamp != lastCamera) {
            cameraFrames++; lastCamera = cameraTimestamp
        }
        val work = (end - start - updateNs).coerceAtLeast(0)
        workNs += work; maxWorkNs = maxOf(maxWorkNs, work)
        if (end - start > 50_000_000L) slowFrames++
        val elapsed = end - started
        if (elapsed < 5_000_000_000L) return null
        val summary = "AR ${cameraFrames * 1_000_000_000L / elapsed} camera fps · " +
            "${frames * 1_000_000_000L / elapsed} draw fps · " +
            "work ${workNs / frames / 1_000_000} ms avg / ${maxWorkNs / 1_000_000} ms max · " +
            "$slowFrames frames >50 ms"
        started = end; frames = 0; cameraFrames = 0; workNs = 0; maxWorkNs = 0; slowFrames = 0
        return summary
    }
}
