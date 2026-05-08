"""
gen_chirp.py - Generate a linear chirp signal (s16 mono WAV).

CLI:
    python3 gen_chirp.py --out chirp.wav --duration 0.2 --f0 100 --f1 8000
                         --sr 48000 --amplitude 0.7

The signal is a linear chirp swept from f0 to f1 Hz over the duration,
with 5 ms Hanning fade-in/fade-out at each end to suppress click artifacts.
"""

import argparse
import sys
import numpy as np
import soundfile as sf
from scipy.signal import chirp


def generate_chirp(
    duration: float = 0.2,
    f0: float = 100.0,
    f1: float = 8000.0,
    sr: int = 48000,
    amplitude: float = 0.7,
) -> np.ndarray:
    """Return a linear chirp as a float64 numpy array in [-1, 1]."""
    t = np.linspace(0, duration, int(sr * duration), endpoint=False)
    signal = amplitude * chirp(t, f0=f0, f1=f1, t1=duration, method="linear")

    # 5 ms Hanning fade-in / fade-out
    fade_samples = int(0.005 * sr)
    if fade_samples > 0 and 2 * fade_samples <= len(signal):
        window = np.hanning(2 * fade_samples)
        signal[:fade_samples] *= window[:fade_samples]
        signal[-fade_samples:] *= window[fade_samples:]

    return signal.astype(np.float32)


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate a linear chirp WAV file.")
    parser.add_argument("--out", required=True, help="Output WAV path")
    parser.add_argument("--duration", type=float, default=0.2, help="Duration in seconds (default 0.2)")
    parser.add_argument("--f0", type=float, default=100.0, help="Start frequency Hz (default 100)")
    parser.add_argument("--f1", type=float, default=8000.0, help="End frequency Hz (default 8000)")
    parser.add_argument("--sr", type=int, default=48000, help="Sample rate Hz (default 48000)")
    parser.add_argument("--amplitude", type=float, default=0.7, help="Peak amplitude 0-1 (default 0.7)")
    args = parser.parse_args()

    signal = generate_chirp(
        duration=args.duration,
        f0=args.f0,
        f1=args.f1,
        sr=args.sr,
        amplitude=args.amplitude,
    )

    sf.write(args.out, signal, args.sr, subtype="PCM_16")
    n_samples = len(signal)
    print(f"Written: {args.out}")
    print(f"  {n_samples} samples @ {args.sr} Hz, {n_samples / args.sr * 1000:.1f} ms, "
          f"f0={args.f0:.0f} Hz → f1={args.f1:.0f} Hz")
    print(f"  amplitude={args.amplitude}, s16 mono")


if __name__ == "__main__":
    main()
