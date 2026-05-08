package com.anymic.app.net

import android.util.Log
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Builds anyMic v1 UDP packets and sends them to (host, port).
 *
 * Header layout (12 bytes, big-endian):
 *   [0]     magic        = 0xA1
 *   [1]     version      = 0x10  (major=1, minor=0)
 *   [2]     flags        bit0 = marker
 *   [3]     payload_type = 0x01 (OPUS_48K_MONO)
 *   [4-5]   seq          u16 BE, wraps at 0xFFFF
 *   [6-9]   timestamp    u32 BE, 48kHz sample clock
 *   [10-11] ssrc16       u16 BE, lower 16 bits of session SSRC
 *
 * Thread-safe: send() may be called from any thread; socket is created by
 * start() and closed by close().
 */
class UdpSender(
    private val host: InetAddress,
    private val port: Int,
    private val ssrc16: Int,           // lower 16 bits of SSRC from HelloAck
    private val payloadType: Int = 1,  // 1 = OPUS_48K_MONO
) : AutoCloseable {

    companion object {
        private const val TAG = "UdpSender"
        private const val MAGIC: Byte = 0xA1.toByte()
        private const val VERSION: Byte = 0x10  // major=1, minor=0
        private const val HEADER_SIZE = 12
    }

    @Volatile private var socket: DatagramSocket? = null

    // seq and timestamp are only written from the single streaming coroutine,
    // no synchronisation needed beyond @Volatile for visibility.
    @Volatile private var seq = 0
    @Volatile private var timestamp: Int = (System.nanoTime() and 0xFFFFFFFFL).toInt()

    fun start() {
        socket = DatagramSocket()
        Log.i(TAG, "UdpSender started → $host:$port ssrc16=0x${ssrc16.and(0xFFFF).toString(16)}")
    }

    override fun close() {
        socket?.close()
        socket = null
        Log.i(TAG, "UdpSender closed")
    }

    /**
     * Wrap an Opus payload with the 12-byte anyMic header and send via UDP.
     *
     * @param payload  Opus frame bytes (≤ 1188 bytes)
     * @param marker   true on first frame after silence (sets flags bit 0)
     * @return number of bytes sent (12 header + payload length), or -1 on error
     */
    fun send(payload: ByteArray, marker: Boolean = false): Int {
        val sock = socket ?: run {
            Log.w(TAG, "send() called before start()")
            return -1
        }

        val flags: Byte = if (marker) 0x01 else 0x00
        val totalLen = HEADER_SIZE + payload.size

        val buf = ByteBuffer.allocate(totalLen).order(ByteOrder.BIG_ENDIAN).apply {
            put(MAGIC)
            put(VERSION)
            put(flags)
            put(payloadType.toByte())
            putShort((seq and 0xFFFF).toShort())
            putInt(timestamp)
            putShort((ssrc16 and 0xFFFF).toShort())
            put(payload)
        }.array()

        return try {
            val packet = DatagramPacket(buf, totalLen, host, port)
            sock.send(packet)
            seq = (seq + 1) and 0xFFFF
            totalLen
        } catch (e: Exception) {
            Log.e(TAG, "send() failed: ${e.message}")
            -1
        }
    }

    /**
     * Advance the 48kHz sample-clock timestamp by [samples] samples.
     * Call after each frame send (240 samples for 5ms @ 48kHz).
     */
    fun advanceTimestamp(samples: Int) {
        timestamp = (timestamp.toLong() + samples).and(0xFFFFFFFFL).toInt()
    }

    val currentSeq: Int get() = seq
    val currentTimestamp: Int get() = timestamp
}
