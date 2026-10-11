package com.aukilabs.annotator

import kotlin.math.floor

data class Cell(val x: Int, val y: Int, val z: Int)

/** Raw-depth timestamps identify measurements, not the current render frame. */
class DepthFreshness {
    private var lastTimestamp = 0L
    fun accept(timestamp: Long): Boolean {
        if (timestamp <= lastTimestamp) return false
        lastTimestamp = timestamp
        return true
    }
    fun reset() { lastTimestamp = 0L }
}

/** Bounded 10 cm surface-occupancy map in one AR session's OpenGL frame. */
class LocalVoxels {
    val cells = LinkedHashSet<Cell>()
    fun add(point: FloatArray) {
        if (point.size != 3 || point.any { !it.isFinite() || kotlin.math.abs(it) > 999f }) return
        val c = Cell(floor(point[0] * 10.0).toInt(), floor(point[1] * 10.0).toInt(), floor(point[2] * 10.0).toInt())
        if (cells.size < 10000 || c in cells) cells.add(c)
    }
    fun clear() = cells.clear()
    fun centers(): FloatArray {
        val result = FloatArray(cells.size * 3)
        var i = 0
        for (c in cells) {
            result[i++] = (c.x + .5f) * .1f
            result[i++] = (c.y + .5f) * .1f
            result[i++] = (c.z + .5f) * .1f
        }
        return result
    }
    fun cellsJson(): String = buildString(cells.size * 20 + 2) {
        append('[')
        var first = true
        for (c in cells) {
            if (!first) append(',')
            first = false
            append('['); append(c.x); append(','); append(c.y); append(','); append(c.z); append(']')
        }
        append(']')
    }
}
