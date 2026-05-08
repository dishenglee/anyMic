"""
silence_detect.py - Detect silence segments in an audio file.

CLI:
    python3 silence_detect.py --input /tmp/rec.wav
                              --threshold-dbfs -50
                              --min-duration-ms 30

Output (stdout, JSON):
    {
      "silences": [{"start_s": 1.234, "end_s": 1.302, "duration_ms": 68}],
      "count": 1,
      "total_silent_ms": 68
    }

Algorithm
---------
RMS sliding window: 20 ms window, 10 ms hop.
Frames below the dBFS threshold are marked silent.
Contiguous silent frames are merged into segments.
Segments shorter than min_duration_ms are discarded.
"""

import argparse
import json
import math
import numpy as np
import soundfile as sf


DEFAULT_WINDOW_MS = 20.0
DEFAULT_HOP_MS    = 10.0


def _rms_dbfs(chunk: np.ndarray) -> float:
    """Compute RMS level in dBFS (full-scale = 1.0)."""
    rms = math.sqrt(max(float(np.mean(chunk.astype(np.float64) ** 2)), 1e-20))
    return 20 * math.log10(rms)


def detect_silences(
    signal: np.ndarray,
    sr: int,
    threshold_dbfs: float = -50.0,
    min_duration_ms: float = 30.0,
    window_ms: float = DEFAULT_WINDOW_MS,
    hop_ms: float = DEFAULT_HOP_MS,
):
    """
    Return list of silent segments as dicts with start_s, end_s, duration_ms.
    """
    window_samples = max(1, int(window_ms / 1000.0 * sr))
    hop_samples    = max(1, int(hop_ms   / 1000.0 * sr))

    mono = signal if signal.ndim == 1 else signal.mean(axis=1)

    n = len(mono)
    frame_times = []
    frame_silent = []

    pos = 0
    while pos + window_samples <= n:
        chunk = mono[pos : pos + window_samples]
        db = _rms_dbfs(chunk)
        t = pos / sr
        frame_times.append(t)
        frame_silent.append(db < threshold_dbfs)
        pos += hop_samples

    # Merge contiguous silent frames into segments
    silences = []
    in_silence = False
    seg_start = 0.0

    for i, (t, is_silent) in enumerate(zip(frame_times, frame_silent)):
        if is_silent and not in_silence:
            seg_start = t
            in_silence = True
        elif not is_silent and in_silence:
            # End of silence: end time = start of this (non-silent) frame
            seg_end = t
            dur_ms = (seg_end - seg_start) * 1000.0
            if dur_ms >= min_duration_ms:
                silences.append({
                    "start_s": round(seg_start, 4),
                    "end_s":   round(seg_end,   4),
                    "duration_ms": round(dur_ms, 2),
                })
            in_silence = False

    # Handle trailing silence
    if in_silence:
        seg_end = min(frame_times[-1] + window_ms / 1000.0, n / sr)
        dur_ms = (seg_end - seg_start) * 1000.0
        if dur_ms >= min_duration_ms:
            silences.append({
                "start_s": round(seg_start, 4),
                "end_s":   round(seg_end,   4),
                "duration_ms": round(dur_ms, 2),
            })

    return silences


def main() -> None:
    parser = argparse.ArgumentParser(description="Detect silence in a WAV file.")
    parser.add_argument("--input", required=True, help="Input WAV file")
    parser.add_argument("--threshold-dbfs", type=float, default=-50.0,
                        dest="threshold_dbfs",
                        help="Silence threshold in dBFS (default -50)")
    parser.add_argument("--min-duration-ms", type=float, default=30.0,
                        dest="min_duration_ms",
                        help="Minimum silence duration to report, ms (default 30)")
    parser.add_argument("--window-ms", type=float, default=DEFAULT_WINDOW_MS,
                        dest="window_ms", help="RMS window length ms (default 20)")
    parser.add_argument("--hop-ms", type=float, default=DEFAULT_HOP_MS,
                        dest="hop_ms", help="RMS hop length ms (default 10)")
    args = parser.parse_args()

    data, sr = sf.read(args.input, dtype="float32", always_2d=True)

    silences = detect_silences(
        signal=data,
        sr=sr,
        threshold_dbfs=args.threshold_dbfs,
        min_duration_ms=args.min_duration_ms,
        window_ms=args.window_ms,
        hop_ms=args.hop_ms,
    )

    total_ms = sum(s["duration_ms"] for s in silences)
    result = {
        "silences": silences,
        "count": len(silences),
        "total_silent_ms": round(total_ms, 2),
    }
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
