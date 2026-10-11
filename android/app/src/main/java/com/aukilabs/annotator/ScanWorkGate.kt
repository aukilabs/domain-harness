package com.aukilabs.annotator

import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/** One in-flight job, including capture and commit. Resets never free its buffers early. */
internal class ScanWorkGate {
    private var epoch = 0L
    private var busy = false
    private var closed = false
    private val lock = ReentrantLock()

    fun acquire(): Long? {
        // The GL thread must not wait for a worker's JNI submission to finish.
        if (!lock.tryLock()) return null
        try {
            if (closed || busy) return null
            busy = true
            return epoch
        } finally { lock.unlock() }
    }
    fun commit(token: Long, action: () -> Unit): Boolean = lock.withLock {
        if (closed || token != epoch) return@withLock false
        action()
        true
    }
    fun finish() = lock.withLock { busy = false }
    fun invalidate(action: () -> Unit) = lock.withLock { epoch++; action() }
    fun close(action: () -> Unit) = lock.withLock { closed = true; epoch++; action() }
}
