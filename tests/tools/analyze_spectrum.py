"""
analyze_spectrum.py - Frequency-domain analysis: SNR, THD, frequency response deviation.

CLI:
    python3 analyze_spectrum.py --reference sweep.wav --captured /tmp/sweep_rec.wav
                                --band 80 8000

Output (stdout, JSON):
    {"snr_db_inband": 26.3, "thd_percent": 1.8, "frequency_response_max_dev_db": 4.2}

Algorithm
---------
1. Load both signals with soundfile, convert to mono, resample if needed.
2. STFT with nperseg=1024, hop=256 (scipy.signal.stft).
3. Align the two STFTs in time (xcorr on RMS envelopes).
4. Frequency response: per-frame ratio captured/reference in the analysis band,
   averaged over time → max deviation in dB.
5. SNR: per-frame, signal = reference band energy, noise = residual band energy
   (captured - expected reference in the band), averaged over frames.
6. THD: pick one representative time frame near the middle of the sweep,
   find the fundamental, sum harmonics 2..8 energy vs fundamental.
"""

import argparse
import json
import math
import sys
import numpy as np
import soundfile as sf
from scipy.signal import stft, resample_poly
from fractions import Fraction


NPERSEG = 1024
HOP     = 256
NFFT    = 1024


def _load_mono(path: str, target_sr: int | None = None):
    data, sr = sf.read(path, dtype="float32", always_2d=True)
    mono = data.mean(axis=1)
    if target_sr is not None and sr != target_sr:
        r = Fraction(target_sr, sr).limit_denominator(1000)
        mono = resample_poly(mono, r.numerator, r.denominator).astype(np.float32)
        sr = target_sr
    return mono, sr


def _freq_bins(sr: int, nfft: int = NFFT):
    """Return the frequency axis for an STFT with nfft points."""
    return np.fft.rfftfreq(nfft, d=1.0 / sr)


def _band_mask(freqs: np.ndarray, f_lo: float, f_hi: float) -> np.ndarray:
    return (freqs >= f_lo) & (freqs <= f_hi)


def _rms_envelope(signal: np.ndarray, hop: int = HOP) -> np.ndarray:
    """Rough RMS envelope used for time alignment."""
    frames = []
    for start in range(0, len(signal), hop):
        chunk = signal[start : start + hop]
        frames.append(float(np.sqrt(np.mean(chunk ** 2))))
    return np.array(frames, dtype=np.float32)


def _align_delay(ref: np.ndarray, cap: np.ndarray, sr: int, hop: int = HOP) -> int:
    """Return sample-level delay of cap relative to ref using envelope xcorr."""
    env_ref = _rms_envelope(ref, hop)
    env_cap = _rms_envelope(cap, hop)
    # correlate
    from scipy.signal import correlate as _corr
    xc = _corr(env_cap, env_ref, mode="full")
    lag_frames = int(np.argmax(xc)) - (len(env_ref) - 1)
    return lag_frames * hop


def analyze(
    ref: np.ndarray,
    cap: np.ndarray,
    sr: int,
    f_lo: float = 80.0,
    f_hi: float = 8000.0,
):
    # Align captured to reference
    delay = _align_delay(ref, cap, sr)
    if delay >= 0:
        cap_aligned = cap[delay:] if delay < len(cap) else cap
    else:
        cap_aligned = np.concatenate([np.zeros(-delay, dtype=np.float32), cap])

    # Trim to same length
    min_len = min(len(ref), len(cap_aligned))
    ref = ref[:min_len]
    cap_aligned = cap_aligned[:min_len]

    freqs, _, Zref = stft(ref, fs=sr, nperseg=NPERSEG, noverlap=NPERSEG - HOP, nfft=NFFT)
    _,    _, Zcap = stft(cap_aligned, fs=sr, nperseg=NPERSEG, noverlap=NPERSEG - HOP, nfft=NFFT)

    mask = _band_mask(freqs, f_lo, f_hi)
    mask_idx = np.where(mask)[0]

    # Magnitude spectra
    Mref = np.abs(Zref)  # shape (n_freq, n_frames)
    Mcap = np.abs(Zcap)

    # ---- Frequency response deviation ----------------------------------------
    # Per-frame: ratio cap/ref in band (dB), then find max deviation from mean
    eps = 1e-12
    # Sum band energy per frame
    ref_band = Mref[mask_idx, :].sum(axis=0)  # (n_frames,)
    cap_band = Mcap[mask_idx, :].sum(axis=0)

    # Only use frames where reference has sufficient energy
    energy_threshold = ref_band.max() * 0.01
    valid = ref_band > energy_threshold
    if not np.any(valid):
        fr_max_dev_db = 0.0
    else:
        ratio_db = 20 * np.log10((cap_band[valid] + eps) / (ref_band[valid] + eps))
        mean_db = float(ratio_db.mean())
        fr_max_dev_db = float(np.max(np.abs(ratio_db - mean_db)))

    # ---- SNR -----------------------------------------------------------------
    # Signal = reference band power per frame (|Zref|^2)
    # Noise  = excess power in captured relative to reference (power subtraction).
    #   For additive uncorrelated noise n: E[|cap|^2] = E[|ref|^2] + E[|n|^2]
    #   so noise_pwr = cap_pwr - ref_pwr  (per frame, clipped to 0).
    # This is statistically unbiased and avoids the phase-mismatch artefact
    # of computing |cap - ref|^2 directly in the complex STFT domain.
    snr_db_inband: float
    ref_pwr = (Mref[mask_idx, :] ** 2).sum(axis=0)
    cap_pwr = (Mcap[mask_idx, :] ** 2).sum(axis=0)
    noise_pwr = np.maximum(cap_pwr - ref_pwr, 0.0)

    if not np.any(valid):
        snr_db_inband = 0.0
    else:
        sig = ref_pwr[valid].mean()
        noi = noise_pwr[valid].mean()
        if noi < 1e-20:
            snr_db_inband = 99.9
        else:
            snr_db_inband = float(10 * math.log10(sig / (noi + eps)))

    # ---- THD -----------------------------------------------------------------
    # Pick a middle frame where ref energy is high, find fundamental, measure harmonics
    mid_frame = int(np.argmax(ref_band))
    spec_ref_frame = Mref[:, mid_frame]
    spec_cap_frame = Mcap[:, mid_frame]

    # Find fundamental in ref (peak in band)
    band_ref = spec_ref_frame.copy()
    band_ref[~mask] = 0.0
    fund_idx = int(np.argmax(band_ref))
    fund_freq = freqs[fund_idx]

    bin_width = freqs[1] - freqs[0]  # Hz per bin
    half_bw = max(2, int(round(fund_freq * 0.05 / bin_width)))  # ±5% of fundamental

    # Fundamental energy (captured)
    f_lo_idx = max(0, fund_idx - half_bw)
    f_hi_idx = min(len(freqs) - 1, fund_idx + half_bw)
    fundamental_energy = float((spec_cap_frame[f_lo_idx : f_hi_idx + 1] ** 2).sum())

    # Harmonic energy in captured (2nd..8th)
    harmonic_energy = 0.0
    for h in range(2, 9):
        h_freq = fund_freq * h
        if h_freq > sr / 2:
            break
        h_bin = int(round(h_freq / bin_width))
        h_lo = max(0, h_bin - half_bw)
        h_hi = min(len(freqs) - 1, h_bin + half_bw)
        harmonic_energy += float((spec_cap_frame[h_lo : h_hi + 1] ** 2).sum())

    if fundamental_energy < 1e-20:
        thd_percent = 0.0
    else:
        thd_percent = float(math.sqrt(harmonic_energy / fundamental_energy) * 100.0)

    return snr_db_inband, thd_percent, fr_max_dev_db


def main() -> None:
    parser = argparse.ArgumentParser(description="Spectral analysis: SNR, THD, freq response.")
    parser.add_argument("--reference", required=True, help="Reference sweep WAV")
    parser.add_argument("--captured", required=True, help="Captured/recorded WAV")
    parser.add_argument("--band", nargs=2, type=float, default=[80.0, 8000.0],
                        metavar=("F_LO", "F_HI"),
                        help="Analysis band in Hz (default 80 8000)")
    parser.add_argument("--sr", type=int, default=None,
                        help="Force target sample rate (default: use reference sr)")
    args = parser.parse_args()

    f_lo, f_hi = args.band

    ref, ref_sr = _load_mono(args.reference)
    target_sr = args.sr if args.sr is not None else ref_sr
    ref, _ = _load_mono(args.reference, target_sr)
    cap, _ = _load_mono(args.captured, target_sr)

    snr_db, thd_pct, fr_dev = analyze(ref, cap, target_sr, f_lo, f_hi)

    result = {
        "snr_db_inband": round(snr_db, 2),
        "thd_percent": round(thd_pct, 3),
        "frequency_response_max_dev_db": round(fr_dev, 3),
    }
    print(json.dumps(result))


if __name__ == "__main__":
    main()
