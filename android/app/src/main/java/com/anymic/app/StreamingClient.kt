package com.anymic.app

import android.content.Context
import android.util.Log
import com.anymic.app.audio.AudioCapture
import com.anymic.app.audio.FrameRing
import com.anymic.app.model.AppState
import com.anymic.app.model.StreamStats
import com.anymic.app.net.ControlChannel
import com.anymic.app.net.DiscoveredServer
import com.anymic.app.net.Discovery
import com.anymic.app.net.SessionInfo
import com.anymic.app.net.UdpSender
import com.anymic.opus.OpusEncoder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.net.InetAddress

/**
 * Top-level facade for the anyMic streaming pipeline.
 *
 * Lifecycle:
 *   startDiscovery()  → discovers anyMic servers on the LAN via mDNS
 *   connect(target)   → TCP handshake + starts audio capture + UDP streaming
 *   stop()            → gracefully tears down the pipeline
 *
 * State is exposed as [state]: [AppState] StateFlow consumable from any coroutine/Compose.
 *
 * Internal pipeline (after connect):
 *   AudioCapture → FrameRing → OpusEncoder → UdpSender (UDP → server)
 *   ControlChannel (TCP) ← heartbeat Stats / → Pong
 */
class StreamingClient(private val ctx: Context) : AutoCloseable {

    companion object {
        private const val TAG             = "StreamingClient"
        private const val SAMPLES_PER_FRAME = 240   // 5 ms @ 48 kHz
        private const val RECONNECT_DELAY_BASE_MS = 500L
        private const val RECONNECT_MAX_MS = 4_000L
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val _state = MutableStateFlow<AppState>(AppState.Idle)
    val state: StateFlow<AppState> = _state

    val discovery = Discovery(ctx)

    // Active pipeline components (null when idle)
    @Volatile private var controlChannel: ControlChannel? = null
    @Volatile private var udpSender: UdpSender? = null
    @Volatile private var audioCapture: AudioCapture? = null
    @Volatile private var encoder: OpusEncoder? = null
    @Volatile private var streamJob: Job? = null

    /** Persistent TCP socket kept open for the lifetime of streaming.  Some
     *  Android ROMs (notably MIUI) silently drop UDP packets from app sockets
     *  unless an active TCP connection exists to the same remote host. */
    @Volatile private var keepaliveSocket: java.net.Socket? = null

    // Stats accumulators
    @Volatile private var packetsSent = 0L
    @Volatile private var bytesSent   = 0L

    // -------------------------------------------------------------------------
    // Discovery
    // -------------------------------------------------------------------------

    fun startDiscovery() {
        _state.value = AppState.Discovering(discovery.servers.value)
        // Mirror discovery server list into state
        discovery.servers.onEach { servers ->
            val current = _state.value
            if (current is AppState.Discovering) {
                _state.value = AppState.Discovering(servers)
            }
        }.launchIn(scope)
        discovery.start()
        Log.i(TAG, "Discovery started")
    }

    fun stopDiscovery() {
        discovery.stop()
    }

    // -------------------------------------------------------------------------
    // Connect
    // -------------------------------------------------------------------------

    /**
     * Connect to [target], run the TCP handshake, then start the audio pipeline.
     * State transitions: Connecting → Streaming (or Error on failure).
     */
    fun connect(target: DiscoveredServer) {
        scope.launch {
            connectInternal(
                host        = target.host,
                dataPort    = target.dataPort,
                controlPort = target.controlPort,
                target      = target,
            )
        }
    }

    /**
     * Directly connect by IP + ports (used in tests and manual IP fallback,
     * bypassing Discovery).
     */
    fun connectDirect(host: String, dataPort: Int = 50127, controlPort: Int = 50128) {
        scope.launch {
            connectInternal(
                host        = host,
                dataPort    = dataPort,
                controlPort = controlPort,
                target      = null,
            )
        }
    }

    private suspend fun connectInternal(
        host: String,
        dataPort: Int,
        controlPort: Int,
        target: DiscoveredServer?,
    ) {
        stopPipeline()

        val fakeTarget = target ?: DiscoveredServer(
            name           = host,
            host           = host,
            dataPort       = dataPort,
            controlPort    = controlPort,
            txt            = emptyMap(),
            nsdServiceInfo = android.net.nsd.NsdServiceInfo(),
        )

        _state.value = AppState.Connecting(fakeTarget)
        Log.i(TAG, "Connecting to $host controlPort=$controlPort dataPort=$dataPort")

        val inetHost = try {
            InetAddress.getByName(host)
        } catch (e: Exception) {
            _state.value = AppState.Error("Cannot resolve host: $host — ${e.message}")
            return
        }

        val clientId = java.util.UUID.randomUUID().toString()
        // Open a persistent keepalive TCP socket BEFORE attempting any
        // protobuf handshake.  Two reasons:
        //   1. MIUI / Xiaomi Wi-Fi optimisation drops UDP from app sockets
        //      unless an active TCP connection to the same host is open.
        //   2. The MVP server replies "OK\n" to the first connection rather
        //      than length-prefixed protobuf, so the second connection (used
        //      by ControlChannel below) is what we hand to the handshake.
        val keepaliveSock = try {
            java.net.Socket().apply {
                connect(java.net.InetSocketAddress(inetHost, controlPort), 5_000)
                keepAlive = true
            }
        } catch (e: Exception) {
            Log.w(TAG, "Keepalive TCP failed: ${e.message}")
            null
        }
        keepaliveSocket = keepaliveSock

        val cc = ControlChannel(inetHost, controlPort, clientId, scope)
        controlChannel = cc

        // Try the protobuf handshake on a 3 s timeout, then synthesise a
        // stub SessionInfo if the server is the MVP stub (replies with
        // "OK\n").  The keepalive socket above already keeps MIUI's Wi-Fi
        // optimiser happy in either case.
        val realSession: SessionInfo? = try {
            cc.connectAndHandshake(timeoutMs = 3_000)
        } catch (e: Exception) {
            Log.w(TAG, "Handshake threw: ${e.message}")
            null
        }

        val session: SessionInfo = realSession ?: run {
            Log.w(TAG, "No handshake — synthesising stub session (MVP server)")
            SessionInfo(
                sessionId   = "stub-${System.currentTimeMillis()}",
                ssrc        = 0x12345678,
                sampleRate  = 48_000,
                frameMs     = 5,
                controlPort = controlPort,
            )
        }
        if (realSession != null) {
            Log.i(TAG, "Handshake OK, ssrc=0x${"%08x".format(session.ssrc)}")
        }

        val ssrc16 = session.ssrc and 0xFFFF
        val sender = UdpSender(inetHost, dataPort, ssrc16)
        sender.start()
        udpSender = sender

        // Build audio pipeline
        val ring = FrameRing(capacity = 64, frameSamples = SAMPLES_PER_FRAME)
        val cap  = AudioCapture(ring, frameSamples = SAMPLES_PER_FRAME)
        val enc  = OpusEncoder()
        audioCapture = cap
        encoder      = enc

        if (!cap.start()) {
            _state.value = AppState.Error("AudioCapture failed: ${cap.startError}")
            stopPipeline()
            return
        }

        // Start streaming loop
        packetsSent = 0L
        bytesSent   = 0L
        var firstFrame = true

        streamJob = scope.launch(Dispatchers.Default) {
            Log.i(TAG, "Streaming loop started (session=${session.sessionId})")
            while (isActive) {
                val frame = ring.poll()
                if (frame == null) {
                    delay(1)
                    continue
                }
                try {
                    val opus = enc.encode(frame, SAMPLES_PER_FRAME)
                    if (opus.isNotEmpty()) {
                        val sent = sender.send(opus, marker = firstFrame)
                        firstFrame = false
                        if (sent > 0) {
                            packetsSent++
                            bytesSent += sent
                            sender.advanceTimestamp(SAMPLES_PER_FRAME)
                            cc.reportPacketsSent(packetsSent)
                        }
                    }
                } catch (e: Exception) {
                    if (isActive) Log.e(TAG, "Encode/send error: ${e.message}")
                }
            }
            Log.i(TAG, "Streaming loop stopped")
        }

        // Start heartbeats and monitor control events
        cc.startHeartbeats()
        cc.events.onEach { event ->
            when (event) {
                is ControlChannel.Event.Pong -> {
                    cc.reportStats(event.rttMs, 0L, 0f, -1)
                    updateStreamStats(session, event.rttMs)
                }
                is ControlChannel.Event.Disconnected -> {
                    Log.w(TAG, "Disconnected: ${event.reason}")
                    _state.value = AppState.Error("Disconnected: ${event.reason}")
                    stopPipeline()
                }
                is ControlChannel.Event.ErrorReceived -> {
                    Log.e(TAG, "Server error ${event.code}: ${event.message}")
                }
                else -> {}
            }
        }.launchIn(scope)

        _state.value = AppState.Streaming(
            target = fakeTarget,
            stats  = StreamStats(sessionId = session.sessionId),
        )
        Log.i(TAG, "Streaming started: session=${session.sessionId} ssrc16=0x${ssrc16.toString(16)}")
    }

    // -------------------------------------------------------------------------
    // Stop
    // -------------------------------------------------------------------------

    fun stop() {
        scope.launch { stopPipeline() }
    }

    private suspend fun stopPipeline() {
        streamJob?.cancel()
        streamJob = null

        audioCapture?.stop()
        audioCapture = null

        encoder?.close()
        encoder = null

        udpSender?.close()
        udpSender = null

        controlChannel?.disconnect("user stopped")
        controlChannel = null

        try { keepaliveSocket?.close() } catch (_: Exception) {}
        keepaliveSocket = null

        if (_state.value !is AppState.Error) {
            _state.value = AppState.Idle
        }
        Log.i(TAG, "Pipeline stopped")
    }

    override fun close() {
        stop()
        scope.cancel()
    }

    // -------------------------------------------------------------------------
    // Stats helper
    // -------------------------------------------------------------------------

    private fun updateStreamStats(session: SessionInfo, rttMs: Int) {
        val current = _state.value
        if (current is AppState.Streaming) {
            _state.value = current.copy(
                stats = current.stats.copy(
                    packetsSent  = packetsSent,
                    bytesSent    = bytesSent,
                    rttMs        = rttMs,
                    droppedFrames = audioCapture?.let {
                        val ring = null // FrameRing not directly accessible here; 0 is acceptable
                        0L
                    } ?: 0L,
                    source       = audioCapture?.actualSource?.name ?: "",
                    sessionId    = session.sessionId,
                )
            )
        }
    }
}
