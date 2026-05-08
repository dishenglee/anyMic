package com.anymic.app.net

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.anymic.proto.v1.ClientMsg
import com.anymic.proto.v1.Codec
import com.anymic.proto.v1.HelloAck
import com.anymic.proto.v1.OS
import com.anymic.proto.v1.ServerMsg
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Verifies that [ControlChannel] sends a structurally valid Hello protobuf
 * and completes the handshake correctly.
 *
 * A mock TCP server runs on a local ephemeral port; it:
 *   1. Reads the length-prefixed ClientMsg and parses it back.
 *   2. Validates Hello fields (client_id, os, version range, codec_caps, sample_rates).
 *   3. Sends a HelloAck with known values.
 *   4. Reads the Ready confirmation from the client.
 */
@RunWith(AndroidJUnit4::class)
class HandshakeTest {

    @Test
    fun hello_contains_correct_fields_and_handshake_completes() {
        runBlocking {
            val latch         = CountDownLatch(1)
            val capturedHello = AtomicReference<com.anymic.proto.v1.Hello?>(null)
            val capturedReady = AtomicReference<Boolean>(false)

            // --- Mock server ---
            val serverSocket = ServerSocket(0)
            val serverPort   = serverSocket.localPort

            Thread {
                serverSocket.soTimeout = 5_000
                val conn = serverSocket.accept()
                conn.soTimeout = 5_000
                val inp = DataInputStream(conn.getInputStream())
                val out = DataOutputStream(conn.getOutputStream())

                // Read Hello
                val helloLen   = inp.readInt()
                val helloBytes = ByteArray(helloLen).also { inp.readFully(it) }
                val clientMsg  = ClientMsg.parseFrom(helloBytes)
                assertTrue("payload should be hello", clientMsg.hasHello())
                capturedHello.set(clientMsg.hello)

                // Send HelloAck
                val ack = ServerMsg.newBuilder().apply {
                    helloAck = HelloAck.newBuilder().apply {
                        sessionId         = "test-session-id"
                        ssrc              = 0xABCD1234.toInt()
                        chosenCodec       = Codec.OPUS_48K_MONO
                        sampleRate        = 48_000
                        frameMs           = 5
                        negotiatedVersion = 0x10
                        udpPort           = 50127
                        serverTsMs        = System.currentTimeMillis()
                    }.build()
                }.build()
                val ackBytes = ack.toByteArray()
                out.writeInt(ackBytes.size); out.write(ackBytes); out.flush()

                // Read Ready
                val readyLen   = inp.readInt()
                val readyBytes = ByteArray(readyLen).also { inp.readFully(it) }
                val readyMsg   = ClientMsg.parseFrom(readyBytes)
                capturedReady.set(readyMsg.hasReady())

                latch.countDown()
                conn.close()
            }.also { it.isDaemon = true; it.start() }

            // --- Client under test ---
            val scope   = CoroutineScope(SupervisorJob() + Dispatchers.IO)
            val host    = InetAddress.getByName("127.0.0.1")
            val cc      = ControlChannel(host, serverPort, "test-client-id", scope)
            val session = cc.connectAndHandshake(timeoutMs = 5_000)

            assertTrue("Latch timed out", latch.await(5, TimeUnit.SECONDS))

            // Validate returned SessionInfo
            assertNotNull("connectAndHandshake should return SessionInfo", session)
            assertEquals("session id",   "test-session-id", session!!.sessionId)
            assertEquals("ssrc",         0xABCD1234.toInt(), session.ssrc)
            assertEquals("sampleRate",   48_000, session.sampleRate)
            assertEquals("frameMs",      5, session.frameMs)

            // Validate Hello fields
            val hello = capturedHello.get()
            assertNotNull("Hello was captured", hello)
            hello!!
            assertEquals("client_id",         "test-client-id",    hello.clientId)
            assertEquals("os",                OS.ANDROID,           hello.os)
            assertEquals("min_version_major", 1,                    hello.minVersionMajor)
            assertEquals("max_version_major", 1,                    hello.maxVersionMajor)
            assertTrue("codec_caps contains OPUS_48K_MONO",
                hello.codecCapsList.contains(Codec.OPUS_48K_MONO))
            assertTrue("sample_rates contains 48000",
                hello.sampleRatesList.contains(48_000))

            // Ready was sent
            assertTrue("Ready was received by mock server", capturedReady.get())

            serverSocket.close()
        }
    }

    @Test
    fun handshake_fails_gracefully_on_server_error_msg() {
        runBlocking {
            val serverSocket = ServerSocket(0)
            val serverPort   = serverSocket.localPort

            Thread {
                serverSocket.soTimeout = 5_000
                val conn = serverSocket.accept()
                conn.soTimeout = 5_000
                val inp = DataInputStream(conn.getInputStream())
                val out = DataOutputStream(conn.getOutputStream())

                // Drain Hello
                val len = inp.readInt()
                ByteArray(len).also { inp.readFully(it) }

                // Reply with ErrorMsg
                val errMsg = ServerMsg.newBuilder().apply {
                    errorMsg = com.anymic.proto.v1.ErrorMsg.newBuilder().apply {
                        codeValue = 1 // VERSION_MISMATCH
                        message   = "Server supports v1.x only"
                        detail    = "version"
                    }.build()
                }.build()
                val errBytes = errMsg.toByteArray()
                out.writeInt(errBytes.size); out.write(errBytes); out.flush()
                conn.close()
            }.also { it.isDaemon = true; it.start() }

            val scope   = CoroutineScope(SupervisorJob() + Dispatchers.IO)
            val host    = InetAddress.getByName("127.0.0.1")
            val cc      = ControlChannel(host, serverPort, "test-err-client", scope)
            val session = cc.connectAndHandshake(timeoutMs = 5_000)

            assertTrue("Session should be null on error", session == null)
            serverSocket.close()
        }
    }
}
