"""
xcorr_latency.py - Measure end-to-end latency via normalized cross-correlation.

CLI:
    python3 xcorr_latency.py --reference chirp.wav --captured /tmp/recorded.wav [--sr 48000]

Output (stdout, JSON):
    {"latency_ms": 73.2, "peak_correlation": 0.93, "snr_db": 28.5}

Exit code 0  = success
Exit code 1  = low correlation (< 0.5), alignment likely failed

Algorithm
---------
1. Load both files with soundfile
2. Resample captured to reference sr if they differ (scipy.signal.resample_poly)
3. Convert stereo to mono by averaging channels
4. Normalised cross-correlation in FFT domain (mode='full')
   xcorr[k] = (ref ★ cap)[k] / sqrt(E_ref * E_cap)
5. Find peak → sample index → latency in ms
6. SNR: signal energy = peak lobe (main lobe ±3 samples around peak index)
         noise energy  = everything outside ±100 ms of the peak
"""

import argparse
import json
import sys
import math
import numpy as np
import soundfile as sf
from scipy.signal import correlate, resample_poly
from fractions import Fraction


MIN_PEAK_CORRELATION = 0.5
LOBE_HALF_SAMPLES = 3       # half-width of the main lobe for signal energy
SNR_NOISE_GAP_MS  = 100.0   # samples within ±100 ms of peak are excluded from noise


def _load_mono(path: str, target_sr: int | None = None):
    """Load a WAV file, convert to mono float32, optionally resample."""
    data, sr = sf.read(path, dtype="float32", always_2d=True)
    # mono
    mono = data.mean(axis=1)
    # resample if needed
    if target_sr is not None and sr != target_sr:
        r = Fraction(target_sr, sr).limit_denominator(1000)
        mono = resample_poly(mono, r.numerator, r.denominator).astype(np.float32)
        sr = target_sr
    return mono, sr


def compute_xcorr_latency(ref: np.ndarray, cap: np.ndarray, sr: int):
    """
    Return (latency_ms, peak_correlation, snr_db).

    ref: mono reference signal
    cap: mono captured signal (may be longer than ref)
    sr:  sample rate (same for both after resampling)
    """
    # Normalisation factors
    energy_ref = float(np.dot(ref, ref))
    energy_cap = float(np.dot(cap, cap))
    norm = math.sqrt(energy_ref * energy_cap)
    if norm == 0:
        raise ValueError("One of the signals is silent (zero energy).")

    # Full cross-correlation: cap ★ ref (find ref inside cap)
    # xcorr[k] answers: "by how many samples do I shift ref to best match cap?"
    # shape: len(cap) + len(ref) - 1
    xcorr = correlate(cap, ref, mode="full", method="fft") / norm

    # Peak
    peak_idx = int(np.argmax(np.abs(xcorr)))
    peak_val = float(xcorr[peak_idx])

    # Convert index to lag in samples
    # correlate(cap, ref, 'full'): zero-lag is at index len(ref) - 1
    # positive lag = ref appears later in cap → cap is "ahead" of ref
    zero_lag = len(ref) - 1
    lag_samples = peak_idx - zero_lag
    latency_ms = lag_samples / sr * 1000.0

    # SNR calculation
    # Signal energy: main lobe around peak (±LOBE_HALF_SAMPLES)
    lobe_lo = max(0, peak_idx - LOBE_HALF_SAMPLES)
    lobe_hi = min(len(xcorr) - 1, peak_idx + LOBE_HALF_SAMPLES)
    signal_energy = float(np.sum(xcorr[lobe_lo : lobe_hi + 1] ** 2))

    # Noise energy: everything outside ±100 ms of the peak
    gap_samples = int(SNR_NOISE_GAP_MS / 1000.0 * sr)
    noise_lo = peak_idx - gap_samples
    noise_hi = peak_idx + gap_samples
    noise_region = np.concatenate([
        xcorr[: max(0, noise_lo)],
        xcorr[min(len(xcorr), noise_hi) :],
    ])
    if len(noise_region) == 0:
        snr_db = 99.9  # signal dominates the whole buffer
    else:
        noise_energy = float(np.sum(noise_region ** 2))
        if noise_energy == 0:
            snr_db = 99.9
        else:
            snr_db = 10.0 * math.log10(signal_energy / noise_energy)

    return latency_ms, abs(peak_val), snr_db


def main() -> None:
    parser = argparse.ArgumentParser(description="Measure latency via cross-correlation.")
    parser.add_argument("--reference", required=True, help="Reference WAV (chirp)")
    parser.add_argument("--captured", required=True, help="Captured/recorded WAV")
    parser.add_argument("--sr", type=int, default=None,
                        help="Force a target sample rate for resampling (default: use reference sr)")
    args = parser.parse_args()

    ref, ref_sr = _load_mono(args.reference)
    target_sr = args.sr if args.sr is not None else ref_sr
    ref, _ = _load_mono(args.reference, target_sr)
    cap, _ = _load_mono(args.captured, target_sr)

    try:
        latency_ms, peak_corr, snr_db = compute_xcorr_latency(ref, cap, target_sr)
    except ValueError as exc:
        print(json.dumps({"error": str(exc)}), file=sys.stderr)
        sys.exit(1)

    result = {
        "latency_ms": round(latency_ms, 3),
        "peak_correlation": round(peak_corr, 4),
        "snr_db": round(snr_db, 2),
    }
    print(json.dumps(result))

    if peak_corr < MIN_PEAK_CORRELATION:
        print(
            f"WARNING: peak_correlation={peak_corr:.3f} < {MIN_PEAK_CORRELATION} — alignment likely failed.",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
