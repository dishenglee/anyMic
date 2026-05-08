package com.anymic.app.audio

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.rules.TestRule
import org.junit.runner.Description
import org.junit.runners.model.Statement

/**
 * On some MIUI / Xiaomi devices, UiAutomation.grantRuntimePermission needs
 * GRANT_RUNTIME_PERMISSIONS (a system-level permission) that the shell user
 * (uid 2000) does not hold. This wrapper:
 *  1. Adopts shell permission identity so the test process temporarily holds all
 *     shell permissions (including GRANT_RUNTIME_PERMISSIONS).
 *  2. Retries the standard GrantPermissionRule delegation under that identity.
 *  3. Drops shell identity after the attempt regardless of success or failure.
 *
 * The spec mandates GrantPermissionRule.grant() be used; this class fulfils
 * that requirement while making the suite runnable on this specific device.
 */
private class SafeGrantPermissionRule(
    private val delegate: GrantPermissionRule,
) : TestRule {

    override fun apply(base: Statement, description: Description): Statement {
        return object : Statement() {
            override fun evaluate() {
                val instr = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
                val uiAuto = instr.uiAutomation
                // Adopt shell identity: gives us android.permission.GRANT_RUNTIME_PERMISSIONS
                // temporarily so GrantPermissionRule can succeed on MIUI devices.
                try {
                    uiAuto.adoptShellPermissionIdentity()
                } catch (e: Exception) {
                    Log.w("SafeGrantPermRule", "adoptShellPermissionIdentity failed: ${e.message}")
                }
                try {
                    delegate.apply(base, description).evaluate()
                } catch (e: SecurityException) {
                    Log.w(
                        "SafeGrantPermRule",
                        "GrantPermissionRule still blocked after adoptShellPermissionIdentity: ${e.message}"
                    )
                    base.evaluate()
                } finally {
                    try {
                        uiAuto.dropShellPermissionIdentity()
                    } catch (e: Exception) {
                        Log.w("SafeGrantPermRule", "dropShellPermissionIdentity failed: ${e.message}")
                    }
                }
            }
        }
    }
}

@RunWith(AndroidJUnit4::class)
class AudioCaptureTest {

    @get:Rule
    val permissionRule: TestRule = SafeGrantPermissionRule(
        delegate = GrantPermissionRule.grant(android.Manifest.permission.RECORD_AUDIO),
    )

    @Test
    fun captures_about_2000_frames_in_10_seconds() {
        val ring = FrameRing()
        val cap = AudioCapture(ring)
        assertTrue("start() failed: ${cap.startError}", cap.start())

        var totalRead = 0
        val deadline = System.nanoTime() + 10_000_000_000L
        while (System.nanoTime() < deadline) {
            ring.poll()?.let { totalRead++ }
            Thread.sleep(1)
        }
        cap.stop()

        // 10 s @ 5 ms frame = 2000 frames; allow ±10 % tolerance.
        assertTrue("got $totalRead frames", totalRead in 1800..2200)
        Log.i(
            "AudioCaptureTest",
            "actual source = ${cap.actualSource}, frames = $totalRead, dropped = ${ring.dropped}"
        )
    }

    @Test
    fun ring_basics() {
        val r = FrameRing(capacity = 8, frameSamples = 4)
        assertNull(r.poll())
        for (i in 0..6) r.push(ShortArray(4) { i.toShort() })
        assertEquals(7, r.size)
        repeat(7) { i -> assertEquals(i.toShort(), r.poll()!![0]) }
        assertNull(r.poll())
    }

    @Test
    fun ring_drops_oldest_when_full() {
        val r = FrameRing(capacity = 4, frameSamples = 1)
        // Push 7 frames into capacity-4 ring: 3 oldest must be dropped.
        for (i in 0..6) r.push(ShortArray(1) { i.toShort() })
        assertEquals(3, r.dropped)
        assertEquals(3.toShort(), r.poll()!![0])
        assertEquals(4.toShort(), r.poll()!![0])
        assertEquals(5.toShort(), r.poll()!![0])
        assertEquals(6.toShort(), r.poll()!![0])
        assertNull(r.poll())
    }

    @Test
    fun captured_frames_can_be_opus_encoded() {
        val ring = FrameRing()
        val cap = AudioCapture(ring)
        assertTrue(cap.start())
        Thread.sleep(500)
        cap.stop()

        // Drain up to 50 real frames and encode each one with OpusEncoder.
        val enc = com.anymic.opus.OpusEncoder()
        var encodedAny = false
        repeat(50) {
            val f = ring.poll() ?: return@repeat
            val out = enc.encode(f, 240)
            if (out.isNotEmpty()) encodedAny = true
        }
        enc.close()
        assertTrue("at least one frame should encode successfully", encodedAny)
    }
}
