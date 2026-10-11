package com.aukilabs.annotator

object NativeBridge {
    init { System.loadLibrary("annotator_bridge") }
    external fun start(profile: ByteArray, email: ByteArray, password: ByteArray, identity: ByteArray): Boolean
    external fun poll(): ByteArray?
    external fun stop()
    external fun localMap(snapshot: ByteArray?): Boolean
    external fun calibrate(luma: ByteArray, capture: ByteArray): ByteArray?
    external fun depth(sample: ByteArray?)
    external fun pose(sample: ByteArray?)
    external fun cameraDemand(active: Boolean): Boolean
    external fun camera(timestamp: Long, jpeg: ByteArray)
    external fun changeNote(request: ByteArray): Boolean
}
