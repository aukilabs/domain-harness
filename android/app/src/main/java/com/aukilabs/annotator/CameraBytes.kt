package com.aukilabs.annotator

import java.nio.ByteBuffer

/** Bulk row copies for normal Y planes; retain support for padded/pixel-strided planes. */
internal fun copyLuma(source: ByteBuffer, width: Int, height: Int, rowStride: Int, pixelStride: Int, target: ByteArray) {
    require(width > 0 && height > 0 && pixelStride > 0 && rowStride >= (width-1)*pixelStride+1)
    require(target.size == width*height)
    val data = source.duplicate()
    val base = data.position()
    require(base.toLong()+(height-1).toLong()*rowStride+(width-1).toLong()*pixelStride < data.limit())
    for (y in 0 until height) {
        if (pixelStride == 1) {
            data.position(base+y*rowStride)
            data.get(target,y*width,width)
        } else for (x in 0 until width) target[y*width+x] = data.get(base+y*rowStride+x*pixelStride)
    }
}
