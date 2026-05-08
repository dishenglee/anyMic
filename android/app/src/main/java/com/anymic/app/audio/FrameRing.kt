package com.anymic.app.audio

import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * Lock-free SPSC ring of fixed-size short[] frames.
 * Writer (capture thread) pushes; reader (network thread) polls.
 * Capacity is power-of-two for cheap modulo via bitwise AND with mask.
 *
 * head and tail are monotonically increasing integers (never wrapped).
 * Modulo is computed as index & mask, avoiding ABA-style confusion that
 * would arise if both counters wrapped at the same capacity boundary.
 * The difference (head - tail) always gives the current fill count
 * unambiguously across all corner cases.
 */
class FrameRing(
    capacity: Int = 64,
    private val frameSamples: Int = 240,
) {
    // Round capacity up to nearest power of two so mask = capacity-1 works.
    private val actualCapacity: Int = nextPow2(capacity)
    private val mask: Int = actualCapacity - 1

    private val buf: Array<ShortArray> = Array(actualCapacity) { ShortArray(frameSamples) }

    // Monotonically increasing; writer owns head, reader owns tail.
    private val head = AtomicInteger(0) // next slot to write
    private val tail = AtomicInteger(0) // next slot to read

    private val droppedCounter = AtomicLong(0)

    companion object {
        private fun nextPow2(n: Int): Int {
            require(n > 0) { "capacity must be positive" }
            var v = n
            v--
            v = v or (v ushr 1)
            v = v or (v ushr 2)
            v = v or (v ushr 4)
            v = v or (v ushr 8)
            v = v or (v ushr 16)
            v++
            return v
        }
    }

    /**
     * Push a frame into the ring.
     * If the ring is full, the oldest frame is discarded (tail advanced)
     * and [dropped] is incremented before writing the new frame.
     *
     * frame is copied into the ring's internal storage so the caller
     * may reuse its ShortArray without causing data races with the reader.
     */
    fun push(frame: ShortArray) {
        val h = head.get()
        val t = tail.get()
        if (h - t == actualCapacity) {
            // Full: drop oldest by advancing tail.
            tail.incrementAndGet()
            droppedCounter.incrementAndGet()
        }
        // Copy into slot h & mask (defensive copy against caller reuse).
        val slot = h and mask
        System.arraycopy(frame, 0, buf[slot], 0, minOf(frame.size, frameSamples))
        // Release-store: any reader that sees head > t will also see the written data.
        head.incrementAndGet()
    }

    /**
     * Poll one frame. Returns null when the ring is empty.
     * The returned array is a fresh copy — caller may keep and mutate it freely.
     */
    fun poll(): ShortArray? {
        val t = tail.get()
        // Acquire-load: if head > t, the corresponding slot is fully written.
        if (head.get() == t) return null
        val slot = t and mask
        val copy = buf[slot].copyOf()
        tail.incrementAndGet()
        return copy
    }

    val dropped: Long get() = droppedCounter.get()

    /** Number of frames currently in the ring. */
    val size: Int get() = head.get() - tail.get()

    val isEmpty: Boolean get() = head.get() == tail.get()
}
