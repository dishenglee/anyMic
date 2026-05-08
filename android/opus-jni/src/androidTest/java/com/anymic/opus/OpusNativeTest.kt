package com.anymic.opus

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.math.sin

@RunWith(AndroidJUnit4::class)
class OpusNativeTest {

    @Test
    fun version_is_nonempty() {
        val v = OpusNative.libopusVersion()
        assertTrue("Expected version to start with 'libopus', got: $v", v.startsWith("libopus"))
    }

    @Test
    fun encode_silence_produces_dtx_frame() {
        OpusEncoder().use { enc ->
            val pcm = ShortArray(240) // 5ms @ 48k mono, all zero
            val bytes = enc.encode(pcm, 240)
            assertTrue("Expected 1..1500 bytes, got ${bytes.size}", bytes.size in 1..1500)
        }
    }

    @Test
    fun encode_sine_tone() {
        OpusEncoder(bitrate = 32000).use { enc ->
            val pcm = ShortArray(240)
            for (i in pcm.indices) {
                pcm[i] = (sin(2 * Math.PI * 1000 * i / 48000.0) * 0.3 * 32767).toInt().toShort()
            }
            val bytes = enc.encode(pcm, 240)
            assertTrue("Expected 5..200 bytes, got ${bytes.size}", bytes.size in 5..200)
        }
    }
}
