"""
test_tools_self.py - Self-tests for the anyMic test tools.

Tests
-----
1.  chirp autocorrelation → latency_ms ≈ 0 (< 0.1 ms)
2.  100 ms artificial delay → xcorr measures 100 ms ± 0.1 ms
3.  sweep self-compare      → SNR extremely high, freq deviation ≈ 0 dB
4.  sweep × 0.5 amplitude   → freq response deviation ≈ −6 dB uniformly
5.  silence detection in a known signal → 1 segment, length ≈ 100 ms ± 10 ms
6.  sweep + −20 dB noise    → SNR ≈ 20 dB ± 3 dB
"""

import json
import math
import sys
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf
import scipy.signal

# ── ensure tools directory is importable ──────────────────────────────────────
_TOOLS_DIR = Path(__file__).parent.resolve()
_TESTS_DIR = _TOOLS_DIR.parent
if str(_TESTS_DIR) not in sys.path:
    sys.path.insert(0, str(_TESTS_DIR))
if str(_TOOLS_DIR) not in sys.path:
    sys.path.insert(0, str(_TOOLS_DIR))

from tools.xcorr_latency    import compute_xcorr_latency, _load_mono
from tools.analyze_spectrum  import analyze
from tools.silence_detect    import detect_silences


# ─────────────────────────────────────────────────────────────────────────────
# Helper: generate fixtures inline without subprocess
# ─────────────────────────────────────────────────────────────────────────────

def _make_chirp(sr: int = 48000, duration: float = 0.2, f0: float = 100, f1: float = 8000,
                amplitude: float = 0.7) -> np.ndarray:
    from tools.gen_chirp import generate_chirp
    return generate_chirp(duration=duration, f0=f0, f1=f1, sr=sr, amplitude=amplitude)


def _make_sweep(sr: int = 48000, duration: float = 5.0) -> np.ndarray:
    from tools.gen_sweep import generate_sweep
    return generate_sweep(duration=duration, sr=sr, amplitude=0.7)


# ─────────────────────────────────────────────────────────────────────────────
# Test 1 – Chirp autocorrelation latency ≈ 0
# ─────────────────────────────────────────────────────────────────────────────

def test_chirp_autocorrelation_zero_latency():
    sr = 48000
    chirp = _make_chirp(sr=sr)
    latency_ms, peak_corr, snr_db = compute_xcorr_latency(chirp, chirp, sr)
    assert abs(latency_ms) < 0.1, (
        f"Self-correlation latency should be < 0.1 ms, got {latency_ms:.4f} ms"
    )
    assert peak_corr > 0.99, f"Self-correlation peak should be ~1.0, got {peak_corr:.4f}"


# ─────────────────────────────────────────────────────────────────────────────
# Test 2 – Artificial 100 ms delay
# ─────────────────────────────────────────────────────────────────────────────

def test_xcorr_100ms_delay():
    sr = 48000
    chirp = _make_chirp(sr=sr)
    delay_ms = 100.0
    delay_samples = int(delay_ms / 1000.0 * sr)

    # captured = silence_prefix + chirp + silence_suffix
    cap = np.concatenate([
        np.zeros(delay_samples, dtype=np.float32),
        chirp,
        np.zeros(delay_samples, dtype=np.float32),
    ])

    latency_ms, peak_corr, _ = compute_xcorr_latency(chirp, cap, sr)
    assert abs(latency_ms - delay_ms) < 0.1, (
        f"Expected {delay_ms} ms delay, measured {latency_ms:.4f} ms "
        f"(error={abs(latency_ms - delay_ms):.4f} ms)"
    )
    assert peak_corr > 0.5, f"Peak correlation too low: {peak_corr:.4f}"


# ─────────────────────────────────────────────────────────────────────────────
# Test 3 – Sweep self-compare: SNR very high, freq deviation ≈ 0
# ─────────────────────────────────────────────────────────────────────────────

def test_sweep_self_compare():
    sr = 48000
    sweep = _make_sweep(sr=sr)
    snr_db, thd_pct, fr_dev = analyze(sweep, sweep, sr, f_lo=80.0, f_hi=8000.0)
    assert snr_db > 30.0, f"Self SNR should be > 30 dB, got {snr_db:.2f} dB"
    assert fr_dev < 1.0, (
        f"Self freq-response deviation should be < 1 dB, got {fr_dev:.3f} dB"
    )


# ─────────────────────────────────────────────────────────────────────────────
# Test 4 – Sweep × 0.5 → freq response deviation ≈ −6 dB uniformly
# ─────────────────────────────────────────────────────────────────────────────

def test_sweep_half_amplitude_freq_response():
    sr = 48000
    sweep = _make_sweep(sr=sr)
    half = (sweep * 0.5).astype(np.float32)
    snr_db, thd_pct, fr_dev = analyze(sweep, half, sr, f_lo=80.0, f_hi=8000.0)
    # The ratio is uniformly -6.02 dB; max deviation from mean should be small
    assert fr_dev < 2.0, (
        f"Half-amplitude should show uniform −6 dB shift (low deviation), "
        f"got max_dev={fr_dev:.3f} dB"
    )


# ─────────────────────────────────────────────────────────────────────────────
# Test 5 – Silence detection: 100 ms known silence
# ─────────────────────────────────────────────────────────────────────────────

def test_silence_detection_100ms():
    sr = 48000
    chirp = _make_chirp(sr=sr, duration=0.5)
    silence_ms = 100.0
    silence_samples = int(silence_ms / 1000.0 * sr)
    silence = np.zeros(silence_samples, dtype=np.float32)

    signal_2d = np.concatenate([chirp, silence, chirp]).reshape(-1, 1)
    silences = detect_silences(
        signal=signal_2d,
        sr=sr,
        threshold_dbfs=-50.0,
        min_duration_ms=30.0,
    )
    assert len(silences) == 1, (
        f"Expected 1 silence segment, got {len(silences)}: {silences}"
    )
    detected_ms = silences[0]["duration_ms"]
    assert abs(detected_ms - silence_ms) <= 10.0, (
        f"Expected ~{silence_ms} ms silence, got {detected_ms:.1f} ms"
    )


# ─────────────────────────────────────────────────────────────────────────────
# Test 6 – Noisy sweep: SNR ≈ 20 dB ± 3 dB
# ─────────────────────────────────────────────────────────────────────────────

def test_sweep_noisy_snr_20db():
    sr = 48000
    f_lo, f_hi = 80.0, 8000.0
    sweep = _make_sweep(sr=sr, duration=3.0)  # shorter for speed

    # Add band-limited noise at −20 dB relative to the sweep RMS.
    # Using bandlimited (80–8000 Hz) noise ensures the in-band SNR equals the
    # intended −20 dB ratio, avoiding the ~4–5 dB in-band gain that wideband
    # white noise shows when only a fraction of it falls inside the analysis band.
    rng = np.random.default_rng(42)
    sweep_rms = float(np.sqrt(np.mean(sweep ** 2)))
    target_snr_linear = 10 ** (-20.0 / 20.0)  # 0.1

    # Build wideband noise then bandlimit it to [f_lo, f_hi] via FFT zeroing
    raw_noise = rng.normal(0, 1.0, size=len(sweep)).astype(np.float64)
    N = len(raw_noise)
    fft_noise = np.fft.rfft(raw_noise)
    freqs_noise = np.fft.rfftfreq(N, d=1.0 / sr)
    band_mask = (freqs_noise >= f_lo) & (freqs_noise <= f_hi)
    fft_noise[~band_mask] = 0.0
    bandlim_noise = np.fft.irfft(fft_noise, n=N).astype(np.float32)

    # Scale bandlimited noise so its RMS = sweep_rms × 10^(−20/20)
    bl_rms = float(np.sqrt(np.mean(bandlim_noise ** 2)))
    if bl_rms > 0:
        bandlim_noise *= (sweep_rms * target_snr_linear) / bl_rms

    captured = (sweep + bandlim_noise).astype(np.float32)

    snr_db, _, _ = analyze(sweep, captured, sr, f_lo=f_lo, f_hi=f_hi)
    # Allow ±3 dB around 20 dB target
    assert abs(snr_db - 20.0) <= 3.0, (
        f"Expected SNR ≈ 20 dB (±3 dB), got {snr_db:.2f} dB"
    )
