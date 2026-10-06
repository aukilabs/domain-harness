package com.aukilabs.annotator

object NativeBridge {
    init { System.loadLibrary("annotator_bridge") }
    external fun start(profile: ByteArray, email: ByteArray, password: ByteArray, identity: ByteArray): Boolean
    external fun poll(): ByteArray?
    external fun stop()
    external fun localMap(snapshot: ByteArray?): Boolean
}
