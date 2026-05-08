package com.anymic.app.net

import android.util.Log
import com.anymic.proto.v1.ClientMsg
import com.anymic.proto.v1.Codec
import com.anymic.proto.v1.Disconnect
import com.anymic.proto.v1.Hello
import com.anymic.proto.v1.OS
import com.anymic.proto.v1.Ready
import com.anymic.proto.v1.ServerMsg
import com.anymic.proto.v1.Stats
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.net.InetAddress
import java.net.Socket
import java.net.SocketTimeoutException

/**
 * TCP control channel implementing the anyMic v1 length-prefixed protobuf framing.
 *
 * Wire format per message: [4-byte BE uint32 length][protobuf bytes]
 *
 * Lifecycle:
 *   1. connectAndHandshake()  — TCP connect + Hello/HelloAck/Ready exchange
 *   2. startHeartbeats()      — sends Stats every 1 s, listens for Pong
 *   3. disconnect()           — sends Disconnect, closes socket
 */
class ControlChannel(
    private val host: InetAddress,
    private val port: Int,
    private val clientId: String,
    private val scope: CoroutineScope,
) {
    companion object {
        private const val TAG = "ControlChannel"
        private const val CONNECT_TIMEOUT_MS = 5_000
        private const val READ_TIMEOUT_MS    = 10_000
        private const val MAX_MSG_BYTES      = 65_535
    }

    sealed class Event {
        data class HandshakeOk(
            val sessionId: String,
            val ssrc: Int,
            val sampleRate: Int,
            val frameMs: Int,
        ) : Event()
        data class HandshakeFailed(val reason: String) : Event()
        data class Pong(val rttMs: Int) : Event()
        data class Disconnected(val reason: String) : Event()
        data class ErrorReceived(val code: Int, val message: String) : Event()
    }

    private val _events = MutableSharedFlow<Event>(extraBufferCapacity = 32)
    val events: SharedFlow<Event> = _events

    @Volatile private var socket: Socket? = null
    @Volatile private var out: DataOutputStream? = null
    @Volatile private var inp: DataInputStream? = null
    @Volatile private var heartbeatJob: Job? = null
    @Volatile private var readerJob: Job? = null

    // Stats tracking for heartbeat
    @Volatile private var statsPktSent: Long = 0
    @Volatile private var statsPktLost: Long = 0
    @Volatile private var statsRttMs: Int = -1
    @Volatile private var statsJitterMs: Float = 0f
    @Volatile private var statsBatteryPct: Int = -1
    @Volatile private var statsSessionStartMs: Long = 0L

    // Pong RTT tracking
    @Volatile private var lastStatsSentMs: Long = 0L

    /**
     * Establish TCP connection, run Hello → HelloAck → Ready handshake.
     * Returns [SessionInfo] on success, null on failure.
     * Emits [Event.HandshakeOk] or [Event.HandshakeFailed] to [events].
     */
    suspend fun connectAndHandshake(timeoutMs: Long = 5_000): SessionInfo? =
        withContext(Dispatchers.IO) {
            val result = withTimeoutOrNull(timeoutMs) {
                try {
                    val sock = Socket()
                    sock.connect(java.net.InetSocketAddress(host, port), CONNECT_TIMEOUT_MS)
                    sock.soTimeout = READ_TIMEOUT_MS
                    socket = sock
                    out = DataOutputStream(sock.getOutputStream())
                    inp = DataInputStream(sock.getInputStream())
                    Log.i(TAG, "TCP connected to $host:$port")

                    // Send Hello
                    val hello = ClientMsg.newBuilder().apply {
                        hello = Hello.newBuilder().apply {
                            clientId = this@ControlChannel.clientId
                            displayName = android.os.Build.MODEL
                            os = OS.ANDROID
                            minVersionMajor = 1
                            maxVersionMajor = 1
                            addCodecCaps(Codec.OPUS_48K_MONO)
                            addSampleRates(48_000)
                        }.build()
                    }.build()
                    writeMessage(hello.toByteArray())
                    Log.d(TAG, "Hello sent, clientId=$clientId")

                    // Read HelloAck (or ErrorMsg)
                    val serverBytes = readMessage()
                        ?: return@withTimeoutOrNull null
                    val serverMsg = ServerMsg.parseFrom(serverBytes)

                    when {
                        serverMsg.hasHelloAck() -> {
                            val ack = serverMsg.helloAck
                            Log.i(TAG, "HelloAck: session=${ack.sessionId} ssrc=${ack.ssrc} " +
                                "sampleRate=${ack.sampleRate} frameMs=${ack.frameMs} " +
                                "udpPort=${ack.udpPort}")

                            // Send Ready
                            val ready = ClientMsg.newBuilder().apply {
                                ready = Ready.newBuilder().build()
                            }.build()
                            writeMessage(ready.toByteArray())
                            Log.d(TAG, "Ready sent")

                            val info = SessionInfo(
                                sessionId   = ack.sessionId,
                                ssrc        = ack.ssrc.toInt(),
                                sampleRate  = ack.sampleRate.toInt(),
                                frameMs     = ack.frameMs.toInt(),
                                controlPort = port,
                            )
                            _events.emit(Event.HandshakeOk(
                                info.sessionId, info.ssrc, info.sampleRate, info.frameMs))
                            info
                        }
                        serverMsg.hasErrorMsg() -> {
                            val err = serverMsg.errorMsg
                            val reason = "Server error ${err.code}: ${err.message}"
                            Log.e(TAG, reason)
                            _events.emit(Event.HandshakeFailed(reason))
                            null
                        }
                        serverMsg.hasDisconnect() -> {
                            val reason = "Server disconnected: ${serverMsg.disconnect.reason}"
                            Log.e(TAG, reason)
                            _events.emit(Event.HandshakeFailed(reason))
                            null
                        }
                        else -> {
                            val reason = "Unexpected server message during handshake"
                            Log.e(TAG, reason)
                            _events.emit(Event.HandshakeFailed(reason))
                            null
                        }
                    }
                } catch (e: SocketTimeoutException) {
                    val reason = "Handshake timed out: ${e.message}"
                    Log.e(TAG, reason)
                    _events.emit(Event.HandshakeFailed(reason))
                    null
                } catch (e: IOException) {
                    val reason = "IO error during handshake: ${e.message}"
                    Log.e(TAG, reason)
                    _events.emit(Event.HandshakeFailed(reason))
                    null
                }
            }

            if (result == null) {
                _events.emit(Event.HandshakeFailed("Handshake timed out after ${timeoutMs}ms"))
            }
            result
        }

    /**
     * Start the heartbeat loop (Stats every [interval] ms) and the reader loop (Pong/Disconnect).
     * Must be called after [connectAndHandshake] succeeds.
     */
    fun startHeartbeats(interval: Long = 1_000L) {
        statsSessionStartMs = System.currentTimeMillis()

        // Reader coroutine: listens for server Pong / Disconnect / ErrorMsg
        readerJob = scope.launch(Dispatchers.IO) {
            val stream = inp ?: return@launch
            while (isActive) {
                try {
                    val bytes = readMessage() ?: break
                    val msg = ServerMsg.parseFrom(bytes)
                    when {
                        msg.hasPong() -> {
                            val pong = msg.pong
                            val now = System.currentTimeMillis()
                            val rtt = (now - lastStatsSentMs).toInt().coerceAtLeast(0)
                            statsRttMs = rtt
                            _events.emit(Event.Pong(rtt))
                            Log.v(TAG, "Pong received, RTT=${rtt}ms")
                        }
                        msg.hasDisconnect() -> {
                            val reason = msg.disconnect.reason
                            Log.w(TAG, "Server sent Disconnect: $reason")
                            _events.emit(Event.Disconnected(reason))
                            break
                        }
                        msg.hasErrorMsg() -> {
                            val err = msg.errorMsg
                            Log.e(TAG, "ErrorMsg from server: ${err.code} ${err.message}")
                            _events.emit(Event.ErrorReceived(err.codeValue, err.message))
                        }
                        else -> Log.v(TAG, "Unhandled server message: $msg")
                    }
                } catch (e: SocketTimeoutException) {
                    // soTimeout fires — ignore, loop continues while active
                } catch (e: IOException) {
                    if (isActive) {
                        Log.e(TAG, "Reader IO error: ${e.message}")
                        _events.emit(Event.Disconnected("IO error: ${e.message}"))
                    }
                    break
                }
            }
        }

        // Sender coroutine: emits Stats every interval ms
        heartbeatJob = scope.launch(Dispatchers.IO) {
            while (isActive) {
                delay(interval)
                try {
                    val nowMs = System.currentTimeMillis()
                    val elapsedMs = nowMs - statsSessionStartMs
                    lastStatsSentMs = nowMs
                    val statsMsg = ClientMsg.newBuilder().apply {
                        stats = Stats.newBuilder().apply {
                            rttMs          = statsRttMs
                            packetsSent    = statsPktSent
                            packetsLost    = statsPktLost
                            jitterMs       = statsJitterMs.toInt().coerceAtLeast(0)
                            batteryPct     = statsBatteryPct
                            inputLevelDbfs = 0
                            clientTsMs     = elapsedMs
                        }.build()
                    }.build()
                    writeMessage(statsMsg.toByteArray())
                    Log.v(TAG, "Stats sent: pkts=$statsPktSent rtt=${statsRttMs}ms")
                } catch (e: IOException) {
                    if (isActive) Log.e(TAG, "Heartbeat IO error: ${e.message}")
                    break
                }
            }
        }
    }

    /** Update stats that will be included in the next heartbeat Stats message. */
    fun reportStats(rttMs: Int, lost: Long, jitterMs: Float, batteryPct: Int) {
        statsRttMs     = rttMs
        statsPktLost   = lost
        statsJitterMs  = jitterMs
        statsBatteryPct = batteryPct
    }

    /** Update packet-sent counter for the heartbeat. */
    fun reportPacketsSent(count: Long) {
        statsPktSent = count
    }

    /** Send Disconnect, cancel background jobs, and close the socket. */
    suspend fun disconnect(reason: String) = withContext(Dispatchers.IO) {
        heartbeatJob?.cancel()
        readerJob?.cancel()
        try {
            val disconnMsg = ClientMsg.newBuilder().apply {
                disconnect = Disconnect.newBuilder().setReason(reason).build()
            }.build()
            writeMessage(disconnMsg.toByteArray())
            Log.i(TAG, "Disconnect sent: $reason")
        } catch (e: IOException) {
            Log.w(TAG, "Could not send Disconnect: ${e.message}")
        }
        closeSocket()
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    /** Write a length-prefixed protobuf message: [4-byte BE length][bytes]. */
    @Throws(IOException::class)
    private fun writeMessage(bytes: ByteArray) {
        val stream = out ?: throw IOException("Output stream is null")
        synchronized(stream) {
            stream.writeInt(bytes.size)
            stream.write(bytes)
            stream.flush()
        }
    }

    /** Read a length-prefixed message. Returns null on EOF or if length > MAX_MSG_BYTES. */
    @Throws(IOException::class)
    private fun readMessage(): ByteArray? {
        val stream = inp ?: return null
        val length = try {
            stream.readInt()
        } catch (e: java.io.EOFException) {
            return null
        }
        if (length <= 0 || length > MAX_MSG_BYTES) {
            Log.e(TAG, "Illegal message length: $length")
            return null
        }
        val buf = ByteArray(length)
        stream.readFully(buf)
        return buf
    }

    private fun closeSocket() {
        try { socket?.close() } catch (_: IOException) {}
        socket = null
        out = null
        inp = null
    }
}

data class SessionInfo(
    val sessionId: String,
    val ssrc: Int,
    val sampleRate: Int,
    val frameMs: Int,
    val controlPort: Int,
)
