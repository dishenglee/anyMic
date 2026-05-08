package com.anymic.app.audio

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Process
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 48kHz mono PCM_16 capture using AudioRecord.
 * Tries source order: VOICE_RECOGNITION -> UNPROCESSED -> MIC.
 * Pushes 240-sample (5ms) frames into a [FrameRing].
 */
class AudioCapture(
    private val ring: FrameRing,
    private val sampleRate: Int = 48_000,
    private val frameSamples: Int = 240,
) {
    companion object {
        private const val TAG = "AudioCapture"
        private const val JOIN_TIMEOUT_MS = 200L
    }

    enum class Source(val androidConst: Int) {
        VOICE_RECOGNITION(MediaRecorder.AudioSource.VOICE_RECOGNITION),
        UNPROCESSED(MediaRecorder.AudioSource.UNPROCESSED),
        MIC(MediaRecorder.AudioSource.MIC),
    }

    @Volatile var actualSource: Source? = null
        private set

    @Volatile var framesProduced: Long = 0L
        private set

    @Volatile var startError: String? = null
        private set

    private val running = AtomicBoolean(false)
    private var recorder: AudioRecord? = null
    private var thread: Thread? = null

    /**
     * Start audio capture. Tries each Source in order; returns true on success.
     * On failure sets [startError] with a descriptive message and returns false.
     */
    fun start(): Boolean {
        if (running.get()) {
            Log.w(TAG, "start() called while already running")
            return true
        }

        val channelConfig = AudioFormat.CHANNEL_IN_MONO
        val encoding = AudioFormat.ENCODING_PCM_16BIT
        val bytesPerSample = 2 // PCM_16 = 2 bytes per sample

        // At least max(minBufferSize * 2, 4 frames) bytes.
        val minBuf = AudioRecord.getMinBufferSize(sampleRate, channelConfig, encoding)
        val minFrameBuf = frameSamples * 4 * bytesPerSample // 4 frames minimum
        val bufferBytes = if (minBuf > 0) maxOf(minBuf * 2, minFrameBuf) else minFrameBuf

        var selectedRecorder: AudioRecord? = null
        var selectedSource: Source? = null

        for (source in Source.values()) {
            try {
                val rec = AudioRecord(
                    source.androidConst,
                    sampleRate,
                    channelConfig,
                    encoding,
                    bufferBytes,
                )
                if (rec.state == AudioRecord.STATE_INITIALIZED) {
                    selectedRecorder = rec
                    selectedSource = source
                    Log.i(TAG, "AudioRecord initialized with source=$source bufferBytes=$bufferBytes")
                    break
                } else {
                    Log.w(TAG, "AudioRecord not initialized for source=$source state=${rec.state}")
                    rec.release()
                }
            } catch (e: Exception) {
                Log.w(TAG, "AudioRecord creation failed for source=$source: ${e.message}")
            }
        }

        if (selectedRecorder == null || selectedSource == null) {
            val msg = "All AudioRecord sources failed (VOICE_RECOGNITION, UNPROCESSED, MIC)"
            startError = msg
            Log.e(TAG, msg)
            return false
        }

        recorder = selectedRecorder
        actualSource = selectedSource

        try {
            selectedRecorder.startRecording()
        } catch (e: Exception) {
            val msg = "startRecording() threw: ${e.message}"
            startError = msg
            Log.e(TAG, msg)
            selectedRecorder.release()
            recorder = null
            return false
        }

        if (selectedRecorder.recordingState != AudioRecord.RECORDSTATE_RECORDING) {
            val msg = "AudioRecord not in RECORDSTATE_RECORDING after startRecording()"
            startError = msg
            Log.e(TAG, msg)
            selectedRecorder.release()
            recorder = null
            return false
        }

        running.set(true)
        framesProduced = 0L

        thread = Thread({
            Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
            val buf = ShortArray(frameSamples)
            Log.d(TAG, "Capture thread started, source=$selectedSource")

            while (running.get()) {
                val read = selectedRecorder.read(buf, 0, frameSamples)
                when {
                    read == frameSamples -> {
                        ring.push(buf) // FrameRing.push copies internally
                        framesProduced++
                    }
                    read > 0 -> {
                        // Partial read — unexpected but non-fatal; push what we got padded to size
                        Log.v(TAG, "Partial read: $read samples")
                        ring.push(buf)
                        framesProduced++
                    }
                    else -> {
                        // 0, ERROR_INVALID_OPERATION, ERROR_BAD_VALUE, or ERROR_DEAD_OBJECT
                        Log.e(TAG, "AudioRecord.read returned $read — stopping capture")
                        break
                    }
                }
            }

            Log.d(TAG, "Capture thread exiting, framesProduced=$framesProduced")
        }, "anymic-capture")

        thread!!.start()
        return true
    }

    /**
     * Stop audio capture. Sets [running] false, stops and releases AudioRecord,
     * then joins the capture thread (timeout [JOIN_TIMEOUT_MS] ms).
     */
    fun stop() {
        if (!running.compareAndSet(true, false)) return

        val rec = recorder
        if (rec != null) {
            try {
                rec.stop()
            } catch (e: Exception) {
                Log.w(TAG, "recorder.stop() threw: ${e.message}")
            }
            rec.release()
            recorder = null
        }

        thread?.join(JOIN_TIMEOUT_MS)
        thread = null
        Log.i(TAG, "Stopped. source=$actualSource framesProduced=$framesProduced")
    }

    val isRunning: Boolean get() = running.get()

    /** True when AudioRecord is in RECORDSTATE_RECORDING after start(). */
    fun isRecording(): Boolean =
        recorder?.recordingState == AudioRecord.RECORDSTATE_RECORDING
}
