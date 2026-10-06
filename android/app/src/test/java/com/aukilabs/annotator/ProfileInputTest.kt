package com.aukilabs.annotator

import org.junit.Assert.assertArrayEquals
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.InputStream

class ProfileInputTest {
    @Test fun readsPartialStreamsWithoutTruncating() {
        val source = "{\"peer\":\"測試\"}".toByteArray()
        val partial = object : ByteArrayInputStream(source) {
            override fun read(b: ByteArray, off: Int, len: Int): Int = super.read(b, off, minOf(2, len))
        }
        assertArrayEquals(source, readProfileBytes(partial))
    }
    @Test(expected = IllegalArgumentException::class) fun rejectsUnboundedInput() {
        readProfileBytes(object : InputStream() { override fun read(): Int = 65 })
    }
    @Test fun acceptsExactLimit() {
        val source = ByteArray(32768) { 65 }
        assertArrayEquals(source, readProfileBytes(ByteArrayInputStream(source)))
    }
}
