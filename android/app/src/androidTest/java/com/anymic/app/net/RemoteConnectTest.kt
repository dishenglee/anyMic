package com.anymic.app.net

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TestRule
import org.junit.runner.RunWith
import java.net.InetAddress
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * RemoteConnectTest — end-to-end instrumented test that connects to a REAL
 * anyMic mac server at a specific IP address and streams live audio for a
 * configurable duration.
 *
 * Invocation via adb am instrument:
 *   adb shell am instrument -w \
 *     -e class com.anymic.app.net.RemoteConnectTest \
 *     -e host <mac-lan-ip> \
 *     -e dataPort 50127 \
 *     -e controlPort 50128 \
 *     -e durationMs 12000 \
 *     com.anymic.app.test/androidx.test.runner.AndroidJUnitRunner
 *
 * Design notes:
 *   - Does NOT use StreamingClient (main/ code is frozen).
 *   - Directly wires: ControlChannel → UdpSender → AudioCapture → OpusEncoder.
 *   - mDNS Discovery is bypassed: IP/ports taken as -e arguments.
 *   - On MIUI (Xiaomi Android 13), the Wi-Fi subsystem delays or drops
 *     UDP packets from app sockets until there is an active TCP connection
 *     to the same remote host.  We therefore keep a raw TCP socket open
 *     throughout the entire streaming window, even when the protobuf
 *     handshake fails (the server currently sends "OK\n" as an MVP stub).
 *   - The keepalive socket is just a plain java.net.Socket opened before
 *     UdpSender.start() and closed after streaming ends.
 */
@RunWith(AndroidJUnit4::class)
class RemoteConnectTest {

    companion object {
        private const val TAG               = "RemoteConnectTest"
        private const val SAMPLES_PER_FRAME = 240   // 5 ms @ 48 kHz
        private const val DEFAULT_SSRC16    = 0x1234 // used when no HelloAck
    }

    @get:Rule
    val perm: TestRule = SafeGrantPermissionRule(
        GrantPermissionRule.grant(android.Manifest.permission.RECORD_AUDIO)
    )

    @Test
    fun connect_to_specific_server_and_stream() {
        val args        = InstrumentationRegistry.getArguments()
        val host        = requireNotNull(args.getString("host")) {
            "Missing -e host <ip>. Pass the Mac's LAN IP to am instrument."
        }
        val dataPort    = args.getString("dataPort")?.toIntOrNull()    ?: 50127
        val controlPort = args.getString("controlPort")?.toIntOrNull() ?: 50128
        val durationMs  = args.getString("durationMs")?.toLongOrNull() ?: 12_000L

        Log.i(TAG, "RemoteConnectTest: host=$host dataPort=$dataPort " +
                   "controlPort=$controlPort durationMs=${durationMs}ms")

        val hostAddr = InetAddress.getByName(host)

        // ── Step 1: Open a keepalive TCP socket ───────────────────────────────
        // On MIUI/Xiaomi devices, UDP packets from the app are silently dropped
        // until there is an active TCP connection to the same remote host.
        // We open a raw Socket to controlPort before starting UDP and keep it
        // open for the entire streaming window.
        val keepaliveSock = try {
            val s = Socket()
            s.connect(java.net.InetSocketAddress(host, controlPort), 5_000)
            s.soTimeout = (durationMs + 5_000).toInt()
            s.setKeepAlive(true)
            Log.i(TAG, "TCP keepalive socket connected to $host:$controlPort")
            // Read whatever the server sends (the MVP "OK\n" stub) but don't block
            s.getInputStream().available().let { available ->
                if (available > 0) {
                    val buf = ByteArray(available)
                    s.getInputStream().read(buf)
                    Log.d(TAG, "TCP server sent: ${buf.decodeToString().trim()}")
                }
            }
            s
        } catch (e: Exception) {
            Log.w(TAG, "TCP keepalive failed: ${e.message} — UDP may be unreliable on MIUI")
            null
        }

        // ── Step 2: Attempt proper ControlChannel handshake ───────────────────
        // This will fail against the MVP server (sends "OK\n" not protobuf),
        // but we try it anyway so that future server upgrades work automatically.
        val scope    = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        val clientId = "remote-test-${System.nanoTime()}"
        val cc       = ControlChannel(hostAddr, controlPort, clientId, scope)

        val sessionInfo = if (keepaliveSock == null) {
            null  // can't do protobuf handshake without TCP
        } else {
            runBlocking {
                try {
                    // Use a separate ControlChannel connection; keepaliveSock stays open.
                    cc.connectAndHandshake(timeoutMs = 5_000)
                } catch (e: Exception) {
                    Log.w(TAG, "Protobuf handshake failed (expected for MVP server): ${e.message}")
                    null
                }
            }
        }

        val ssrc16 = sessionInfo?.ssrc?.and(0xFFFF) ?: DEFAULT_SSRC16

        if (sessionInfo != null) {
            Log.i(TAG, "Handshake OK: session=${sessionInfo.sessionId} ssrc16=0x${ssrc16.toString(16)}")
        } else {
            Log.w(TAG, "No handshake — using direct UDP ssrc16=0x${ssrc16.toString(16)}")
        }

        // ── Step 3: Build and start the streaming pipeline ────────────────────
        val sender = UdpSender(hostAddr, dataPort, ssrc16)
        sender.start()

        val ring = com.anymic.app.audio.FrameRing(capacity = 64, frameSamples = SAMPLES_PER_FRAME)
        val cap  = com.anymic.app.audio.AudioCapture(ring, frameSamples = SAMPLES_PER_FRAME)
        val enc  = com.anymic.opus.OpusEncoder(
            bitrate = 96_000,
            application = com.anymic.opus.OpusNative.APPLICATION_AUDIO,
        )

        assertTrue("AudioCapture.start() failed: ${cap.startError}", cap.start())
        if (sessionInfo != null) {
            cc.startHeartbeats()
        }

        val streamStop  = AtomicBoolean(false)
        val packetsSent = CountDownLatch(10)

        val streamThread = Thread({
            var firstFrame = true
            while (!streamStop.get()) {
                val frame = ring.poll()
                if (frame == null) { Thread.sleep(1); continue }
                val opus = enc.encode(frame, SAMPLES_PER_FRAME)
                if (opus.isNotEmpty()) {
                    val sent = sender.send(opus, marker = firstFrame)
                    if (sent > 0) {
                        firstFrame = false
                        packetsSent.countDown()
                    }
                    sender.advanceTimestamp(SAMPLES_PER_FRAME)
                }
            }
        }, "anymic-remote-stream")
        streamThread.isDaemon = true
        streamThread.start()

        // ── Step 4: Stream for durationMs ────────────────────────────────────
        Log.i(TAG, "Streaming for ${durationMs}ms… (TCP keepalive=${keepaliveSock != null})")
        val startMs = System.currentTimeMillis()
        while (System.currentTimeMillis() - startMs < durationMs) {
            Thread.sleep(100)
        }

        // ── Step 5: Tear down ─────────────────────────────────────────────────
        streamStop.set(true)
        streamThread.join(500)

        val finalSeq = sender.currentSeq
        cap.stop()
        enc.close()
        sender.close()

        if (sessionInfo != null) {
            runBlocking {
                try { cc.disconnect("test complete") } catch (_: Exception) {}
            }
        }

        // Close keepalive socket AFTER all streaming is done
        try { keepaliveSock?.close() } catch (_: Exception) {}

        Log.i(TAG, "Stream done: udpSeq=$finalSeq framesProduced=${cap.framesProduced}")

        // ── Step 6: Assertions ────────────────────────────────────────────────
        assertTrue(
            "AudioCapture produced 0 frames — mic permission denied? startError=${cap.startError}",
            cap.framesProduced > 0
        )

        val minExpectedFrames = (durationMs / 5) - 100
        assertTrue(
            "Too few frames: ${cap.framesProduced} < $minExpectedFrames",
            cap.framesProduced >= minExpectedFrames
        )

        val gotPackets = packetsSent.await(5, TimeUnit.SECONDS)
        assertTrue(
            "Fewer than 10 UDP packets sent — UdpSender stalled on $host:$dataPort",
            gotPackets
        )

        Log.i(TAG, "PASS: framesProduced=${cap.framesProduced} udpSeq=$finalSeq")
    }
}
