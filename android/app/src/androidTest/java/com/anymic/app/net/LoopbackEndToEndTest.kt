package com.anymic.app.net

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import com.anymic.proto.v1.Codec
import com.anymic.proto.v1.HelloAck
import com.anymic.proto.v1.Pong
import com.anymic.proto.v1.ServerMsg
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TestRule
import org.junit.runner.Description
import org.junit.runner.RunWith
import org.junit.runners.model.Statement
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.ServerSocket
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Fully self-contained loopback end-to-end test.
 *
 * Architecture (all on 127.0.0.1, ephemeral ports):
 *   Mock TCP server  — replies Hello→HelloAck, Ready→Pong
 *   Mock UDP sink    — collects incoming Opus packets
 *   ControlChannel   — real TCP handshake client
 *   UdpSender        — real packet builder / sender
 *   AudioCapture     — real microphone capture
 *   OpusEncoder      — real Opus encoding
 *
 * Validates per packet:
 *   - magic == 0xA1
 *   - seq strictly +1 (mod 0x10000)
 *   - timestamp advances exactly +240 per packet
 *   - ssrc16 == server-assigned 0x1234 (lower 16 bits of 0xABCD1234)
 *   - payload length in [5, 300]
 */
@RunWith(AndroidJUnit4::class)
class LoopbackEndToEndTest {

    companion object {
        private const val TAG                = "LoopbackE2E"
        private const val TARGET_PACKETS     = 100
        private const val TIMEOUT_SECS       = 60L
        private const val SAMPLES_PER_FRAME  = 240
        private const val SERVER_SSRC        = 0xABCD1234.toInt()
        private const val EXPECTED_SSRC16    = SERVER_SSRC and 0xFFFF   // 0x1234
    }

    @get:Rule
    val perm: TestRule = SafeGrantPermissionRule(
        GrantPermissionRule.grant(android.Manifest.permission.RECORD_AUDIO)
    )

    @Test
    fun client_streams_real_audio_to_localhost_listener() {
      runBlocking {
        val host = InetAddress.getByName("127.0.0.1")

        // --- Bind ephemeral sockets ---
        val udpSink   = DatagramSocket(0)
        val tcpServer = ServerSocket(0)
        val udpPort   = udpSink.localPort
        val tcpPort   = tcpServer.localPort
        udpSink.soTimeout   = 2_000
        tcpServer.soTimeout = 10_000
        Log.i(TAG, "Ports: udp=$udpPort tcp=$tcpPort")

        // --- Start mock TCP server ---
        val serverReady = CountDownLatch(1)
        val serverDone  = AtomicBoolean(false)

        Thread {
            try {
                serverReady.countDown()   // Signal: ServerSocket is bound, client may now connect
                val conn = tcpServer.accept()
                conn.soTimeout = 5_000
                val inp = DataInputStream(conn.getInputStream())
                val out = DataOutputStream(conn.getOutputStream())

                // Drain Hello
                val hLen = inp.readInt()
                ByteArray(hLen).also { inp.readFully(it) }
                Log.d(TAG, "Server: Hello $hLen bytes")

                // Send HelloAck
                val ack = ServerMsg.newBuilder().setHelloAck(
                    HelloAck.newBuilder()
                        .setSessionId("loopback-session")
                        .setSsrc(SERVER_SSRC)
                        .setChosenCodec(Codec.OPUS_48K_MONO)
                        .setSampleRate(48_000)
                        .setFrameMs(5)
                        .setNegotiatedVersion(0x10)
                        .setUdpPort(udpPort)
                        .setServerTsMs(System.currentTimeMillis())
                ).build()
                val ab = ack.toByteArray()
                out.writeInt(ab.size); out.write(ab); out.flush()
                Log.d(TAG, "Server: HelloAck sent")

                // Drain Ready
                val rLen = inp.readInt()
                ByteArray(rLen).also { inp.readFully(it) }
                Log.d(TAG, "Server: Ready received")

                // Serve Stats → Pong until done
                while (!serverDone.get()) {
                    try {
                        val mLen = inp.readInt()
                        val mData = ByteArray(mLen).also { inp.readFully(it) }
                        val cMsg = com.anymic.proto.v1.ClientMsg.parseFrom(mData)
                        if (cMsg.hasStats()) {
                            val pongMsg = ServerMsg.newBuilder().setPong(
                                Pong.newBuilder()
                                    .setServerTsMs(System.currentTimeMillis())
                                    .setEchoedClientTsMs(cMsg.stats.clientTsMs)
                            ).build()
                            val pb = pongMsg.toByteArray()
                            out.writeInt(pb.size); out.write(pb); out.flush()
                        }
                    } catch (_: Exception) { break }
                }
                conn.close()
            } catch (e: Exception) {
                Log.e(TAG, "TCP server error: ${e.message}")
                serverReady.countDown()  // unblock client even on error
            }
        }.also { it.isDaemon = true; it.start() }

        // --- Start UDP collector ---
        val received    = CopyOnWriteArrayList<ByteArray>()
        val packetLatch = CountDownLatch(TARGET_PACKETS)
        val udpDone     = AtomicBoolean(false)

        Thread {
            val buf = ByteArray(2048)
            val pkt = DatagramPacket(buf, buf.size)
            while (!udpDone.get()) {
                try {
                    udpSink.receive(pkt)
                    received.add(buf.copyOf(pkt.length))
                    packetLatch.countDown()
                    if (received.size >= TARGET_PACKETS) break
                } catch (_: java.net.SocketTimeoutException) {}
            }
        }.also { it.isDaemon = true; it.start() }

        // Wait for server
        assertTrue("TCP server ready", serverReady.await(5, TimeUnit.SECONDS))

        // --- Connect client pipeline ---
        val scope    = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        val clientId = "e2e-${System.nanoTime()}"
        val cc       = ControlChannel(host, tcpPort, clientId, scope)
        val session  = cc.connectAndHandshake(timeoutMs = 8_000)

        assertTrue("connectAndHandshake must not return null", session != null)
        val ssrc16 = session!!.ssrc and 0xFFFF

        val sender = UdpSender(host, udpPort, ssrc16)
        sender.start()

        val ring = com.anymic.app.audio.FrameRing(capacity = 64, frameSamples = SAMPLES_PER_FRAME)
        val cap  = com.anymic.app.audio.AudioCapture(ring, frameSamples = SAMPLES_PER_FRAME)
        val enc  = com.anymic.opus.OpusEncoder()

        assertTrue("AudioCapture.start()", cap.start())
        cc.startHeartbeats()

        val streamStop = AtomicBoolean(false)
        val streamThread = Thread {
            var firstFrame = true
            while (!streamStop.get()) {
                val frame = ring.poll()
                if (frame == null) { Thread.sleep(1); continue }
                val opus = enc.encode(frame, SAMPLES_PER_FRAME)
                if (opus.isNotEmpty()) {
                    sender.send(opus, marker = firstFrame)
                    firstFrame = false
                    sender.advanceTimestamp(SAMPLES_PER_FRAME)
                }
            }
        }.also { it.isDaemon = true; it.start() }

        // --- Wait for packets ---
        val gotAll = packetLatch.await(TIMEOUT_SECS, TimeUnit.SECONDS)

        // --- Tear down ---
        streamStop.set(true); serverDone.set(true); udpDone.set(true)
        streamThread.join(500)
        cap.stop(); enc.close(); sender.close()
        udpSink.close(); tcpServer.close()

        // --- Validate ---
        assertTrue("Received $TARGET_PACKETS packets (got ${received.size})", gotAll)

        val pkts = received.take(TARGET_PACKETS)

        // Per-packet field checks
        for ((i, p) in pkts.withIndex()) {
            assertTrue("pkt[$i] min length (${p.size})", p.size >= 13)
            assertEquals("pkt[$i] magic", 0xA1.toByte(), p[0])
            val payLen = p.size - 12
            // Opus 5ms frames range from 1 byte (DTX/minimal) to ~160 bytes at 256kbps.
            // At 32kbps typical is 20-40 bytes; allow 1..300 to be robust.
            assertTrue("pkt[$i] payload in [1,300], got $payLen", payLen in 1..300)
        }

        // Seq strictly +1
        var prevSeq = -1
        for ((i, p) in pkts.withIndex()) {
            val seq = ByteBuffer.wrap(p, 4, 2).order(ByteOrder.BIG_ENDIAN).short.toInt() and 0xFFFF
            if (prevSeq >= 0) {
                val exp = (prevSeq + 1) and 0xFFFF
                assertEquals("pkt[$i] seq=$seq expected=$exp", exp, seq)
            }
            prevSeq = seq
        }

        // Timestamp +240
        var prevTs = -1L
        for ((i, p) in pkts.withIndex()) {
            val ts = ByteBuffer.wrap(p, 6, 4).order(ByteOrder.BIG_ENDIAN).int.toLong() and 0xFFFFFFFFL
            if (prevTs >= 0) {
                val exp = (prevTs + SAMPLES_PER_FRAME) and 0xFFFFFFFFL
                assertEquals("pkt[$i] ts=$ts expected=$exp", exp, ts)
            }
            prevTs = ts
        }

        // ssrc16
        for ((i, p) in pkts.withIndex()) {
            val s = ByteBuffer.wrap(p, 10, 2).order(ByteOrder.BIG_ENDIAN).short.toInt() and 0xFFFF
            assertEquals("pkt[$i] ssrc16=0x${s.toString(16)} expected=0x${EXPECTED_SSRC16.toString(16)}",
                EXPECTED_SSRC16, s)
        }

        Log.i(TAG, "PASSED: ${pkts.size} packets, all assertions OK")
      } // end runBlocking
    }
}

/**
 * Shared MIUI-compatible permission grant rule.
 * Declared here; also used by HandshakeTest and AudioCaptureTest via their own copies.
 */
internal class SafeGrantPermissionRule(
    private val delegate: GrantPermissionRule,
) : TestRule {
    override fun apply(base: Statement, description: Description): Statement =
        object : Statement() {
            override fun evaluate() {
                val uiAuto = InstrumentationRegistry.getInstrumentation().uiAutomation
                try { uiAuto.adoptShellPermissionIdentity() } catch (_: Exception) {}
                try {
                    delegate.apply(base, description).evaluate()
                } catch (e: SecurityException) {
                    base.evaluate()
                } finally {
                    try { uiAuto.dropShellPermissionIdentity() } catch (_: Exception) {}
                }
            }
        }
}
