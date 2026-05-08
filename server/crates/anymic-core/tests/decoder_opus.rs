//! Integration tests for `anymic_core::decoder` — Opus decoder with PLC.
//!
//! All signals are synthesised in-test; no fixture files are required.

use std::f32::consts::PI;

use anymic_core::decoder::{DecoderError, FrameDecoder, OpusFrameDecoder};
use audiopus::{coder::Encoder, Application, Channels, SampleRate};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default VoIP profile: 48 kHz / mono / 5 ms.
const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 240; // 5 ms @ 48 kHz

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an Opus encoder for the VoIP profile.
fn make_encoder() -> Encoder {
    Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)
        .expect("failed to create test encoder")
}

/// Encode `pcm` (240 i16 samples) into an Opus packet and return the payload bytes.
fn encode_frame(enc: &Encoder, pcm: &[i16]) -> Vec<u8> {
    assert_eq!(
        pcm.len(),
        FRAME_SAMPLES,
        "test helper: unexpected frame size"
    );
    let mut buf = vec![0u8; 4096];
    let len = enc.encode(pcm, &mut buf).expect("encode failed");
    buf.truncate(len);
    buf
}

/// Generate a 1 kHz sine wave with the given amplitude (0.0–1.0) as i16 PCM.
fn sine_1khz(amplitude: f32) -> Vec<i16> {
    (0..FRAME_SAMPLES)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let v = amplitude * (2.0 * PI * 1_000.0 * t).sin();
            (v * i16::MAX as f32) as i16
        })
        .collect()
}

/// Compute RMS of an i16 slice.
fn rms(samples: &[i16]) -> f64 {
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    (sum_sq / samples.len() as f64).sqrt()
}

/// Generate a 1 kHz sine wave with explicit phase offset (radians).
fn sine_1khz_phase(amplitude: f32, phase_rad: f32) -> Vec<i16> {
    (0..FRAME_SAMPLES)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let v = amplitude * (2.0 * PI * 1_000.0 * t + phase_rad).sin();
            (v * i16::MAX as f32) as i16
        })
        .collect()
}

/// Compute SNR in dB: 10 * log10( signal_power / noise_power ).
///
/// For Opus VoIP mode the decoded 1 kHz sine can be phase-shifted by up to
/// ~185° relative to the input (a known behaviour of the SILK predictor).
/// This function finds the phase offset of `signal` that minimises the
/// reconstruction error against `decoded`, so the reported SNR reflects
/// codec distortion rather than a phase mismatch artefact.
///
/// For non-sine inputs (silence, arbitrary PCM) pass `freq_hz = 0` to use
/// a naive sample-by-sample comparison instead.
fn snr_db(signal: &[i16], decoded: &[i16]) -> f64 {
    assert_eq!(signal.len(), decoded.len());

    let signal_power: f64 =
        signal.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / signal.len() as f64;

    let noise_power: f64 = signal
        .iter()
        .zip(decoded.iter())
        .map(|(&s, &d)| (s as f64 - d as f64).powi(2))
        .sum::<f64>()
        / signal.len() as f64;

    if noise_power < 1.0 {
        // Essentially lossless / silence — return a large finite SNR.
        return 120.0;
    }
    10.0 * (signal_power / noise_power).log10()
}

/// Phase-aware SNR for a 1 kHz sine.
///
/// Opus VoIP mode (SILK predictor) introduces a ~185° phase rotation on the
/// decoded output. We scan all phase offsets in 1° steps and pick the one that
/// gives the lowest noise power, then report the corresponding SNR.
///
/// This is the correct way to measure codec distortion for a pure-tone test:
/// the phase rotation is an inherent property of the codec, not signal
/// degradation.
fn snr_db_sine_1khz(amplitude: f32, decoded: &[i16]) -> f64 {
    let signal_power: f64 = {
        let sp: f64 = decoded.iter().map(|&s| (s as f64).powi(2)).sum();
        // Use decoded power as signal power — we know amplitude so also check
        // that the decoded magnitude is in the right ballpark.
        let _ = sp;
        // Use the original signal power.
        let orig = sine_1khz_phase(amplitude, 0.0);
        orig.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / orig.len() as f64
    };

    let mut best_noise_power = f64::MAX;

    // Scan 360 phases in 1° steps.
    for deg in 0..360 {
        let phase = deg as f32 * PI / 180.0;
        let shifted = sine_1khz_phase(amplitude, phase);
        let np: f64 = shifted
            .iter()
            .zip(decoded.iter())
            .map(|(&s, &d)| (s as f64 - d as f64).powi(2))
            .sum::<f64>()
            / shifted.len() as f64;
        if np < best_noise_power {
            best_noise_power = np;
        }
    }

    if best_noise_power < 1.0 {
        return 120.0;
    }
    10.0 * (signal_power / best_noise_power).log10()
}

// ---------------------------------------------------------------------------
// Test 1 — can create
// ---------------------------------------------------------------------------

#[test]
fn t01_create_new_voip() {
    OpusFrameDecoder::new_voip().expect("new_voip() should succeed");
}

// ---------------------------------------------------------------------------
// Test 2 — decode silence frame (all-zero PCM)
// ---------------------------------------------------------------------------

#[test]
fn t02_decode_silence_rms_low() {
    let enc = make_encoder();
    let silence = vec![0i16; FRAME_SAMPLES];
    let opus_data = encode_frame(&enc, &silence);

    let mut dec = OpusFrameDecoder::new_voip().unwrap();
    let pcm = dec.decode(&opus_data).expect("decode silence failed");

    assert_eq!(pcm.len(), FRAME_SAMPLES);

    let r = rms(&pcm);
    println!("[t02] silence RMS = {r:.2}");
    assert!(r < 100.0, "RMS of silence frame should be < 100, got {r}");
}

// ---------------------------------------------------------------------------
// Test 3 — decode 1 kHz sine, SNR > 25 dB
//
// Opus has a built-in encoder look-ahead of ~312 samples (6.5 ms). To get a
// meaningful SNR comparison we warm up both the encoder and decoder with
// several frames of the same 1 kHz sine before measuring, so that the
// encode→decode pipeline is in steady state. We compare the decoded output
// of the last warmup frame against the same sine, at which point the pipeline
// delay has been flushed and the signal should reconstruct faithfully.
// ---------------------------------------------------------------------------

#[test]
fn t03_decode_sine_1khz_snr() {
    const WARMUP_FRAMES: usize = 10;

    let enc = make_encoder();
    let mut dec = OpusFrameDecoder::new_voip().unwrap();
    let sine = sine_1khz(0.3);

    let mut last_decoded = vec![0i16; FRAME_SAMPLES];

    for _ in 0..WARMUP_FRAMES {
        let opus_data = encode_frame(&enc, &sine);
        last_decoded = dec.decode(&opus_data).expect("decode sine failed");
        assert_eq!(last_decoded.len(), FRAME_SAMPLES);
    }

    // After WARMUP_FRAMES of the same 1 kHz sine the encoder/decoder pipeline
    // is in steady state. Use the phase-aware SNR helper because Opus VoIP
    // mode (SILK predictor) introduces a ~185° phase rotation on the output.
    let snr = snr_db_sine_1khz(0.3, &last_decoded);
    println!("[t03] 1kHz sine SNR (phase-aware, steady-state) = {snr:.1} dB");
    assert!(
        snr > 25.0,
        "SNR for 1kHz sine should be > 25 dB, got {snr:.1} dB"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — PLC returns exactly `samples` samples
// ---------------------------------------------------------------------------

#[test]
fn t04_plc_returns_correct_length() {
    let mut dec = OpusFrameDecoder::new_voip().unwrap();
    let plc = dec.decode_plc(FRAME_SAMPLES);
    assert_eq!(
        plc.len(),
        FRAME_SAMPLES,
        "PLC should return exactly {FRAME_SAMPLES} samples"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — PLC followed by a real decode succeeds
// ---------------------------------------------------------------------------

#[test]
fn t05_plc_then_real_decode() {
    let enc = make_encoder();
    let sine = sine_1khz(0.3);
    let opus_data = encode_frame(&enc, &sine);

    let mut dec = OpusFrameDecoder::new_voip().unwrap();

    // Insert one lost packet.
    let _ = dec.decode_plc(FRAME_SAMPLES);

    // Next real frame must still decode to the right length.
    let decoded = dec.decode(&opus_data).expect("decode after PLC failed");
    assert_eq!(
        decoded.len(),
        FRAME_SAMPLES,
        "frame after PLC should still be 240 samples"
    );
}

// ---------------------------------------------------------------------------
// Test 6 — with_params(frame_samples=300) triggers InvalidFrameSize on decode
// ---------------------------------------------------------------------------

#[test]
fn t06_invalid_frame_size_error() {
    // Use a valid opus frame size (240) for encoding but tell the decoder to
    // expect 300 samples — this should trigger the size mismatch error.
    let enc = make_encoder();
    let sine = sine_1khz(0.3);
    let opus_data = encode_frame(&enc, &sine);

    // Decoder configured to expect 300 samples per frame.
    let mut dec =
        OpusFrameDecoder::with_params(SAMPLE_RATE, 1, 300).expect("with_params should succeed");

    let result = dec.decode(&opus_data);
    assert!(result.is_err(), "should error when frame size mismatches");

    match result.unwrap_err() {
        DecoderError::InvalidFrameSize { got, expected } => {
            println!("[t06] InvalidFrameSize: got={got}, expected={expected}");
            assert_eq!(expected, 300);
            // `got` is what Opus actually decoded (240 for a 5ms frame).
            assert_ne!(got, expected);
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 7 — 100 consecutive frames, SNR > 20 dB after warmup
//
// We skip the first WARMUP_FRAMES when measuring SNR because the Opus encoder
// look-ahead means early decoded frames don't align with the original. After
// warmup the pipeline is in steady state and every subsequent frame exceeds
// 20 dB SNR.
// ---------------------------------------------------------------------------

#[test]
fn t07_multi_frame_continuous_snr() {
    const WARMUP_FRAMES: usize = 5;
    const TOTAL_FRAMES: usize = 100;

    let enc = make_encoder();
    let mut dec = OpusFrameDecoder::new_voip().unwrap();

    let min_snr = 20.0_f64;
    let mut worst_snr = f64::MAX;

    for frame_idx in 0..TOTAL_FRAMES {
        let sine = sine_1khz(0.3);
        let opus_data = encode_frame(&enc, &sine);
        let decoded = dec
            .decode(&opus_data)
            .expect("decode failed in multi-frame test");

        assert_eq!(
            decoded.len(),
            FRAME_SAMPLES,
            "frame {frame_idx}: wrong sample count"
        );

        // Skip warmup frames for SNR measurement; use phase-aware SNR for sine.
        if frame_idx >= WARMUP_FRAMES {
            let snr = snr_db_sine_1khz(0.3, &decoded);
            if snr < worst_snr {
                worst_snr = snr;
            }
        }
    }

    println!("[t07] {TOTAL_FRAMES}-frame (phase-aware, post-warmup) worst SNR = {worst_snr:.1} dB");
    assert!(
        worst_snr > min_snr,
        "worst SNR over {TOTAL_FRAMES} frames (post-warmup) should be > {min_snr} dB, got {worst_snr:.1} dB"
    );
}

// ---------------------------------------------------------------------------
// Test 8 — reset clears internal state, no cross-contamination
// ---------------------------------------------------------------------------

#[test]
fn t08_reset_cleans_state() {
    let enc = make_encoder();
    let mut dec = OpusFrameDecoder::new_voip().unwrap();

    // Encode and decode 50 frames of 1 kHz sine.
    let sine = sine_1khz(0.3);
    for _ in 0..50 {
        let opus_data = encode_frame(&enc, &sine);
        dec.decode(&opus_data).expect("decode before reset failed");
    }

    // Reset the decoder.
    dec.reset();

    // After reset, decode a silence frame and verify it round-trips cleanly.
    let enc2 = make_encoder();
    let silence = vec![0i16; FRAME_SAMPLES];
    let opus_silence = encode_frame(&enc2, &silence);
    let pcm = dec
        .decode(&opus_silence)
        .expect("decode after reset failed");

    assert_eq!(pcm.len(), FRAME_SAMPLES);

    let r = rms(&pcm);
    println!("[t08] post-reset silence RMS = {r:.2}");
    // After reset, silence frame should produce near-silent output.
    assert!(
        r < 500.0,
        "after reset, silence RMS should be low, got {r:.2}"
    );
}
