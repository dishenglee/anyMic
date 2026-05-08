"""
gen_speech.py - Generate a synthetic substitute voice (s16 mono WAV).

CLI:
    python3 gen_speech.py --out speech.wav --duration 10.0 --sr 48000

Produces 5 vowels (a/e/i/o/u), each ~2 s, with:
  - A fundamental frequency (F0) in the speech range 100-200 Hz + vibrato
  - Two/three formant resonances shaped with 2nd-order IIR bandpass filters
  - Soft amplitude envelope (no abrupt clicks between vowels)

Energy is concentrated 80-4000 Hz, mimicking speech-like spectral shape.
This is a test substitute - for real PESQ / MOS evaluation replace with a
genuine ITU-T P.50 or similar reference speech file (see fixtures/README.md).
"""

import argparse
import numpy as np
import soundfile as sf
from scipy.signal import iirpeak, lfilter


# Formant frequencies (F1, F2, F3) and bandwidths (Hz) for each vowel
_VOWELS = {
    "a": {"f0": 120, "formants": [(800, 80), (1200, 100), (2500, 150)]},
    "e": {"f0": 140, "formants": [(400, 60), (2200, 120), (2800, 140)]},
    "i": {"f0": 160, "formants": [(300, 50), (2700, 130), (3300, 160)]},
    "o": {"f0": 110, "formants": [(500, 70), (800, 90),  (2500, 150)]},
    "u": {"f0": 100, "formants": [(300, 50), (600, 80),  (2200, 130)]},
}


def _formant_filter(signal: np.ndarray, freq: float, bw: float, sr: int) -> np.ndarray:
    """Apply a 2nd-order bandpass (peaking) filter at `freq` Hz with bandwidth `bw` Hz."""
    Q = freq / bw
    b, a = iirpeak(freq / (sr / 2), Q)
    return lfilter(b, a, signal)


def generate_vowel(
    vowel_key: str,
    duration: float,
    sr: int,
    amplitude: float = 0.6,
) -> np.ndarray:
    spec = _VOWELS[vowel_key]
    f0 = spec["f0"]
    n = int(sr * duration)
    t = np.arange(n) / sr

    # Fundamental: sawtooth-like harmonic stack (voiced source)
    vibrato_rate = 5.5  # Hz
    vibrato_depth = 3.0  # Hz peak deviation
    f0_mod = f0 + vibrato_depth * np.sin(2 * np.pi * vibrato_rate * t)
    phase = np.cumsum(2 * np.pi * f0_mod / sr)

    # Mix harmonics to approximate a voiced source
    source = np.zeros(n)
    for k in range(1, 12):
        source += (1.0 / k) * np.sin(k * phase)
    source /= np.max(np.abs(source) + 1e-9)

    # Apply formant filters
    filtered = source.copy()
    for freq, bw in spec["formants"]:
        filtered = _formant_filter(filtered, freq, bw, sr)

    # Normalize and apply amplitude
    peak = np.max(np.abs(filtered))
    if peak > 0:
        filtered /= peak
    filtered *= amplitude

    # Soft envelope: 20 ms fade in/out
    fade = int(0.02 * sr)
    win = np.hanning(2 * fade)
    filtered[:fade] *= win[:fade]
    filtered[-fade:] *= win[fade:]

    return filtered.astype(np.float32)


def generate_speech(duration: float = 10.0, sr: int = 48000, amplitude: float = 0.6) -> np.ndarray:
    """Generate concatenated synthetic vowels totalling `duration` seconds."""
    vowel_keys = list(_VOWELS.keys())
    n_vowels = len(vowel_keys)
    per_vowel = duration / n_vowels

    segments = []
    for key in vowel_keys:
        seg = generate_vowel(key, per_vowel, sr, amplitude)
        segments.append(seg)

    return np.concatenate(segments)


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate synthetic substitute speech WAV.")
    parser.add_argument("--out", required=True, help="Output WAV path")
    parser.add_argument("--duration", type=float, default=10.0, help="Total duration in seconds (default 10.0)")
    parser.add_argument("--sr", type=int, default=48000, help="Sample rate Hz (default 48000)")
    parser.add_argument("--amplitude", type=float, default=0.6, help="Peak amplitude 0-1 (default 0.6)")
    args = parser.parse_args()

    signal = generate_speech(duration=args.duration, sr=args.sr, amplitude=args.amplitude)
    sf.write(args.out, signal, args.sr, subtype="PCM_16")
    n_samples = len(signal)
    print(f"Written: {args.out}")
    print(f"  {n_samples} samples @ {args.sr} Hz, {n_samples / args.sr:.2f} s, 5 synthetic vowels")
    print(f"  amplitude={args.amplitude}, s16 mono")
    print("NOTE: This is a synthetic substitute. For real PESQ testing, replace with")
    print("      an ITU-T P.50 reference file (see fixtures/README.md).")


if __name__ == "__main__":
    main()
