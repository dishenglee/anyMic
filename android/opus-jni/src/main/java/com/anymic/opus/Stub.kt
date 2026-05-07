package com.anymic.opus

/**
 * Opus JNI stub — placeholder until T09 implements the native Opus encoder/decoder bridge.
 *
 * TODO T09: replace with real JNI bindings to libopus compiled via CMake/NDK.
 */
object OpusStub {
    fun encode(pcm: ByteArray): ByteArray {
        throw NotImplementedError("Opus JNI not yet implemented (T09)")
    }

    fun decode(data: ByteArray): ByteArray {
        throw NotImplementedError("Opus JNI not yet implemented (T09)")
    }
}
