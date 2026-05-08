package com.anymic.opus

object OpusNative {
    init { System.loadLibrary("opus_jni") }

    @JvmStatic external fun createEncoder(sampleRate: Int, channels: Int, application: Int): Long
    @JvmStatic external fun destroyEncoder(handle: Long)
    @JvmStatic external fun setBitrate(handle: Long, bitrate: Int): Int
    @JvmStatic external fun encode(handle: Long, pcm: ShortArray, samplesPerChannel: Int, out: ByteArray, outMaxLen: Int): Int
    @JvmStatic external fun libopusVersion(): String

    const val APPLICATION_VOIP = 2048   // OPUS_APPLICATION_VOIP
    const val APPLICATION_AUDIO = 2049  // OPUS_APPLICATION_AUDIO (broadcast / music — fullband)
}

class OpusEncoder(
    sampleRate: Int = 48000,
    channels: Int = 1,
    bitrate: Int = 32000,
    application: Int = OpusNative.APPLICATION_VOIP,
) : AutoCloseable {
    private var handle: Long = OpusNative.createEncoder(sampleRate, channels, application)

    init {
        require(handle != 0L) { "opus_encoder_create failed" }
        OpusNative.setBitrate(handle, bitrate)
    }

    fun encode(pcm: ShortArray, samplesPerChannel: Int): ByteArray {
        val out = ByteArray(1500)
        val n = OpusNative.encode(handle, pcm, samplesPerChannel, out, out.size)
        require(n >= 0) { "opus encode error: $n" }
        return out.copyOf(n)
    }

    override fun close() {
        if (handle != 0L) {
            OpusNative.destroyEncoder(handle)
            handle = 0L
        }
    }
}
