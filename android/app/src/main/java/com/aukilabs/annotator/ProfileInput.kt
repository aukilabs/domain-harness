package com.aukilabs.annotator

import java.io.InputStream

internal fun readProfileBytes(stream: InputStream): ByteArray {
    val buffer = ByteArray(32769)
    var used = 0
    while (used < buffer.size) {
        val count = stream.read(buffer, used, buffer.size - used)
        if (count < 0) break
        if (count == 0) {
            val byte = stream.read()
            if (byte < 0) break
            buffer[used++] = byte.toByte()
        } else used += count
    }
    require(used <= 32768) { "Connection profile is too large" }
    return buffer.copyOf(used)
}
