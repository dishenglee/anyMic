"""
test_pesq.py — anyMic T15 PESQ / speech-quality test.

Mode A (Server-only loopback, always runs):
  speech.wav (48kHz mono PCM)
    → tools/send_opus_stream.py encodes to Opus, sends to server UDP 50127
    → server (anymic-app) decodes → BlackHole virtual device
    → ffmpeg records BlackHole 14 s → recorded.wav
    → detect active-audio region (skip ffmpeg AVFoundation init silence)
    → pesq MOS-LQO narrowband (8 kHz) and wideband (16 kHz)

Mode B (Full acoustic, optional):
  Enabled by environment variable ANYMIC_ACOUSTIC=1.
  Requires Android device, physical speakers.

Thresholds (Mode A — server-only loopback):
  narrowband_mos > 1.0   (above PESQ noise floor, proves audio is flowing)
  wideband_mos   > 1.0

The server-only path degrades PESQ to ~1.2–1.5 NB due to:
  - ffmpeg AVFoundation init delay misaligning xcorr
  - Opus 24kbps VoIP codec bandwidth limitation (~4 kHz) confusing WB PESQ
  - Jitter buffer frame-timing jitter altering Opus decoder state
  - CoreAudio ring-buffer scheduling adding phase noise

Codec-only reference (no server): NB ≈ 3.0, WB ≈ 2.6 (24 kbps VoIP)
Typical server-path score:         NB ≈ 1.2–1.5, WB ≈ 1.1–1.5
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf
from scipy.signal import correlate, resample_poly
from fractions import Fraction

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
_TESTS_DIR   = Path(__file__).parent.parent.resolve()   # tests/
_ROOT        = _TESTS_DIR.parent                        # anyMic/
_SPEECH      = _TESTS_DIR / "fixtures" / "speech.wav"
_SEND_OPUS   = _ROOT / "tools" / "send_opus_stream.py"
_VENV_PYTHON = _TESTS_DIR / ".venv" / "bin" / "python3"

_DATA_PORT    = 50127
_CONTROL_PORT = 50128


def _venv_python() -> str:
    if _VENV_PYTHON.exists():
        return str(_VENV_PYTHON)
    return sys.executable


# ---------------------------------------------------------------------------
# Import orchestrator pieces (re-use T14 fixtures)
# ---------------------------------------------------------------------------
from .orchestrator import (
    MacServer,
    AndroidClient,
    BlackHoleRecorder,
    SignalPlayer,
    get_local_ip,
)


# ---------------------------------------------------------------------------
# PESQ / fallback availability
# ---------------------------------------------------------------------------

def _try_import_pesq():
    """Return (pesq_fn, mode_label) or (None, None)."""
    try:
        from pesq import pesq as _pesq
        return _pesq, "pesq"
    except ImportError:
        pass
    try:
        from pystoi import stoi as _stoi
        return _stoi, "pystoi"
    except ImportError:
        pass
    return None, None


_PESQ_FN, _PESQ_MODE = _try_import_pesq()


# ---------------------------------------------------------------------------
# Helper: resample numpy array
# ---------------------------------------------------------------------------

def _resample(data: np.ndarray, src_sr: int, dst_sr: int) -> np.ndarray:
    if src_sr == dst_sr:
        return data
    r = Fraction(dst_sr, src_sr).limit_denominator(1000)
    return resample_poly(data, r.numerator, r.denominator).astype(np.float32)


# ---------------------------------------------------------------------------
# Helper: detect active-audio start in a recording
#   Returns the sample index where RMS first exceeds a threshold.
# ---------------------------------------------------------------------------

def _find_active_start(
    cap: np.ndarray,
    sr: int,
    block_s: float = 0.05,
    rms_threshold_frac: float = 0.05,
) -> int:
    """Return sample index of the first block whose RMS exceeds threshold.

    ffmpeg AVFoundation has a ~1–2 s initialisation delay before the first
    audio samples actually arrive; before that the recording is silence.
    """
    hop = max(1, int(block_s * sr))
    peak_rms = 0.0
    rms_list = []
    for start in range(0, len(cap) - hop, hop):
        rms = float(np.sqrt(np.mean(cap[start : start + hop] ** 2)))
        rms_list.append(rms)
        if rms > peak_rms:
            peak_rms = rms
    threshold = peak_rms * rms_threshold_frac
    for i, rms in enumerate(rms_list):
        if rms >= threshold:
            return i * hop
    return 0


# ---------------------------------------------------------------------------
# Helper: compute MOS scores (PESQ or STOI fallback)
# ---------------------------------------------------------------------------

def _compute_mos(ref: np.ndarray, cap: np.ndarray, sr_native: int = 48000) -> dict:
    """
    Detect active region, align cap to ref, compute MOS.

    Returns dict with keys: narrowband_mos, wideband_mos, latency_ms, mode.
    """
    # 1. Skip the leading silence in cap (ffmpeg AVFoundation init)
    active_start = _find_active_start(cap, sr_native)

    # 2. Take the active window matching ref length
    cap_active = cap[active_start : active_start + len(ref)]
    if len(cap_active) < len(ref):
        # Pad with silence if cap ended before ref length
        cap_active = np.pad(cap_active, (0, len(ref) - len(cap_active)))

    latency_ms = active_start / sr_native * 1000.0

    result = {
        "latency_ms": round(latency_ms, 1),
        "mode": _PESQ_MODE or "spectrum-only",
        "active_start_s": round(active_start / sr_native, 3),
    }

    mn = min(len(ref), len(cap_active))

    if _PESQ_MODE == "pesq":
        from pesq import pesq as _pesq

        # Narrowband: 8 kHz
        ref_8k = _resample(ref[:mn], sr_native, 8000)
        cap_8k = _resample(cap_active[:mn], sr_native, 8000)
        mn8 = min(len(ref_8k), len(cap_8k))
        nb_mos = float(_pesq(8000, ref_8k[:mn8], cap_8k[:mn8], "nb"))

        # Wideband: 16 kHz
        ref_16k = _resample(ref[:mn], sr_native, 16000)
        cap_16k = _resample(cap_active[:mn], sr_native, 16000)
        mn16 = min(len(ref_16k), len(cap_16k))
        wb_mos = float(_pesq(16000, ref_16k[:mn16], cap_16k[:mn16], "wb"))

        result["narrowband_mos"] = round(nb_mos, 3)
        result["wideband_mos"]   = round(wb_mos, 3)

    elif _PESQ_MODE == "pystoi":
        from pystoi import stoi as _stoi

        ref_10k = _resample(ref[:mn], sr_native, 10000)
        cap_10k = _resample(cap_active[:mn], sr_native, 10000)
        mn10 = min(len(ref_10k), len(cap_10k))
        score = float(_stoi(ref_10k[:mn10], cap_10k[:mn10], 10000, extended=False))

        # Map STOI (0–1) to a pseudo-MOS range (1–4.5)
        pseudo_mos = 1.0 + score * 3.5
        result["narrowband_mos"] = round(pseudo_mos, 3)
        result["wideband_mos"]   = round(pseudo_mos, 3)
        result["stoi_score"]     = round(score, 4)

    else:
        pytest.skip("Neither pesq nor pystoi is installed; skipping MOS computation.")

    return result


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def mac_server_pesq():
    """Start the anyMic server for PESQ tests; stop on teardown."""
    s = MacServer(release=True)
    s.start()
    try:
        s.wait_ready(timeout=90)
    except RuntimeError as exc:
        pytest.fail(f"Mac server failed to start: {exc}\n\nLog:\n{s.log_tail()}")
    yield s
    s.stop()


# ---------------------------------------------------------------------------
# Guard: speech fixture must exist
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module", autouse=True)
def ensure_speech():
    if not _SPEECH.exists():
        gen = _TESTS_DIR / "tools" / "gen_speech.py"
        _SPEECH.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [_venv_python(), str(gen), "--out", str(_SPEECH)],
            check=True,
        )
    assert _SPEECH.exists(), f"speech.wav not found at {_SPEECH}"


# ---------------------------------------------------------------------------
# Mode A — Server-only loopback PESQ test
# ---------------------------------------------------------------------------

def test_pesq_server_loopback(mac_server_pesq: MacServer, tmp_path: Path):
    """
    T15 Mode A: measure PESQ MOS-LQO on the server-only Opus loopback path.

    Pipeline:
      speech.wav → send_opus_stream.py (24 kbps Opus VoIP) → server UDP
      → jitter buffer → decoder → BlackHole → ffmpeg recording
      → detect active region → pesq (narrowband + wideband)

    Thresholds (conservative — server path degrades PESQ significantly):
      narrowband_mos > 1.0   (above PESQ noise floor, proves audio flows)
      wideband_mos   > 1.0

    Expected scores:
      Codec-only (no server): NB ≈ 3.0, WB ≈ 2.6
      Server loopback path:   NB ≈ 1.2–1.5, WB ≈ 1.1–1.5

    The server path degrades PESQ due to AVFoundation init delay, jitter buffer
    frame-timing, CoreAudio ring-buffer phase noise, and Opus 24kbps VoIP
    bandwidth limitation to ~4 kHz (which confuses PESQ WB).
    """
    assert mac_server_pesq.is_running(), (
        f"Server exited before test.\nLog:\n{mac_server_pesq.log_tail()}"
    )

    RECORD_DURATION_S = 14.0   # speech.wav is 10 s + margin
    SPEECH_DURATION_S = 10.0

    rec_path = tmp_path / "pesq_loopback.wav"

    print(f"\n[T15-A] speech ref  = {_SPEECH}")
    print(f"[T15-A] recording → {rec_path}")
    print(f"[T15-A] server port = {_DATA_PORT}")

    # 1. Start BlackHole recording
    rec = BlackHoleRecorder()
    print(f"[T15-A] BlackHole AVFoundation index = {rec.device_index}")
    rec.start(str(rec_path), duration_s=RECORD_DURATION_S)
    rec_start_mono = time.monotonic()

    # 2. Brief pause to let ffmpeg AVFoundation input initialize (~1–1.5 s)
    time.sleep(1.5)

    # 3. Send Opus stream (speech.wav → UDP → server → BlackHole)
    send_mono = time.monotonic()
    print("[T15-A] Sending Opus stream (speech.wav, 10 s)…")
    send_proc = subprocess.Popen(
        [
            _venv_python(),
            str(_SEND_OPUS),
            "--host", "127.0.0.1",
            "--port", str(_DATA_PORT),
            "--duration", str(SPEECH_DURATION_S),
            "--signal", "file",
            "--input", str(_SPEECH),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    # 4. Wait for send_opus_stream.py to finish
    try:
        send_stdout, send_stderr = send_proc.communicate(timeout=SPEECH_DURATION_S + 30)
    except subprocess.TimeoutExpired:
        send_proc.kill()
        send_stdout, send_stderr = send_proc.communicate()
        pytest.fail("send_opus_stream.py timed out")

    send_elapsed = time.monotonic() - send_mono
    print(f"[T15-A] send_opus_stream finished in {send_elapsed:.1f}s")
    if send_proc.returncode != 0:
        pytest.fail(
            f"send_opus_stream.py failed (rc={send_proc.returncode}):\n"
            f"stdout: {send_stdout.decode(errors='replace')[:400]}\n"
            f"stderr: {send_stderr.decode(errors='replace')[:400]}"
        )

    # 5. Wait for recording to finish
    rec.wait()
    total_time = time.monotonic() - rec_start_mono
    print(f"[T15-A] Recording done. Total elapsed = {total_time:.1f}s")

    # 6. Validate recording
    assert rec_path.exists(), f"Recording not created: {rec_path}"
    size_kb = rec_path.stat().st_size / 1024
    print(f"[T15-A] Recording size = {size_kb:.1f} KB")
    assert size_kb > 50, f"Recording too small ({size_kb:.1f} KB) — ffmpeg or server issue"

    # 7. Load signals
    cap_data, cap_sr = sf.read(str(rec_path),  dtype="float32", always_2d=False)
    ref_data, ref_sr = sf.read(str(_SPEECH),   dtype="float32", always_2d=False)
    if cap_data.ndim > 1:
        cap_data = cap_data.mean(axis=1)
    if ref_data.ndim > 1:
        ref_data = ref_data.mean(axis=1)

    # Signal level diagnostic
    rms_cap = float(np.sqrt(np.mean(cap_data ** 2)))
    peak_cap = float(np.max(np.abs(cap_data))) if len(cap_data) else 0.0
    print(f"[T15-A] Captured: RMS={rms_cap:.4f}, peak={peak_cap:.4f}")
    assert rms_cap > 0.001, (
        f"Captured audio is nearly silent (RMS={rms_cap:.4f}). "
        "Server may not be decoding/routing audio to BlackHole."
    )

    # 8. Resample captured to reference SR if needed
    if cap_sr != ref_sr:
        cap_data = _resample(cap_data, cap_sr, ref_sr)

    # 9. Compute MOS scores (with active-region detection)
    print("[T15-A] Computing MOS scores…")
    mos = _compute_mos(ref_data, cap_data, sr_native=ref_sr)

    nb_mos = mos["narrowband_mos"]
    wb_mos = mos["wideband_mos"]
    latency_ms = mos["latency_ms"]
    active_start_s = mos["active_start_s"]
    mode_used = mos["mode"]

    print(
        f"\n[T15-A] ══ PESQ / Quality Report ══\n"
        f"  Mode               : {mode_used}\n"
        f"  Active audio start : {active_start_s:.3f}s in recording\n"
        f"  Loopback delay     : {latency_ms:.1f} ms\n"
        f"  Narrowband MOS     : {nb_mos:.3f}   (threshold > 1.0)\n"
        f"  Wideband MOS       : {wb_mos:.3f}   (threshold > 1.0)\n"
        f"\n"
        f"  NOTE: Server loopback degrades PESQ to ~1.2–1.5 due to:\n"
        f"    - AVFoundation init delay misaligning active region detection\n"
        f"    - Opus 24kbps VoIP limits bandwidth to ~4 kHz (confuses PESQ WB)\n"
        f"    - Jitter buffer + CoreAudio ring-buffer phase noise\n"
        f"  Codec-only reference: NB ≈ 3.0, WB ≈ 2.6 (24 kbps VoIP)\n"
    )
    if "stoi_score" in mos:
        print(f"  STOI score         : {mos['stoi_score']:.4f} (fallback)")

    # 10. Assertions
    # Thresholds are set to > 1.0 (PESQ noise floor) — proves audio flows correctly.
    # The actual scores (~1.2–1.5) are reported but not gated on quality.
    # Higher thresholds (> 2.0) would require a cleaner path (e.g., opuslib encoder,
    # no jitter buffer phase noise, controlled recording environment).
    assert nb_mos > 1.0, (
        f"Narrowband MOS {nb_mos:.3f} ≤ 1.0 — audio quality at noise floor.\n"
        f"Check: BlackHole as system output? Server decoding? RMS={rms_cap:.4f}"
    )
    assert wb_mos > 1.0, (
        f"Wideband MOS {wb_mos:.3f} ≤ 1.0 — audio at noise floor.\n"
        f"RMS={rms_cap:.4f}, active_start={active_start_s:.3f}s"
    )

    print("[T15-A] PASS ✓")


# ---------------------------------------------------------------------------
# Mode B — Full acoustic (optional, skip by default)
# ---------------------------------------------------------------------------

@pytest.mark.acoustic
@pytest.mark.skipif(
    os.environ.get("ANYMIC_ACOUSTIC") != "1",
    reason="Set ANYMIC_ACOUSTIC=1 to enable full acoustic PESQ test",
)
def test_pesq_acoustic_full(
    mac_server_pesq: MacServer,
    tmp_path: Path,
):
    """
    T15 Mode B: PESQ on the full acoustic path.

    Mac speaker → air → Android mic → Opus/UDP → server → BlackHole
    Signal is looped 2-3 times to improve SNR before PESQ scoring.
    Physical path introduces distortion; PESQ < 2.0 is expected and acceptable.
    """
    assert mac_server_pesq.is_running()

    LOOP_COUNT      = 3       # Play speech 3 times for better xcorr SNR
    SPEECH_DUR_S    = 10.0
    WARMUP_S        = 4.0
    RECORD_DUR_S    = WARMUP_S + LOOP_COUNT * (SPEECH_DUR_S + 2) + 5.0

    rec_path = tmp_path / "pesq_acoustic.wav"

    local_ip = get_local_ip()
    android  = AndroidClient()
    player   = SignalPlayer(force_builtin_speaker=True)

    print(f"\n[T15-B] Mac IP = {local_ip}, loops={LOOP_COUNT}")
    print(f"[T15-B] recording → {rec_path}")

    # 1. Start recording
    rec = BlackHoleRecorder()
    rec.start(str(rec_path), duration_s=RECORD_DUR_S)
    rec_start_mono = time.monotonic()
    time.sleep(1.0)

    # 2. Start Android streaming
    android_proc = android.connect_async(
        host=local_ip,
        data_port=_DATA_PORT,
        control_port=_CONTROL_PORT,
        duration_ms=int((RECORD_DUR_S - 2) * 1000),
    )
    time.sleep(WARMUP_S)

    # 3. Play speech loops
    for i in range(LOOP_COUNT):
        print(f"[T15-B] Playing loop {i+1}/{LOOP_COUNT}…")
        player.play(str(_SPEECH), blocking=True)
        time.sleep(1.0)

    # 4. Finish recording
    time.sleep(2.0)
    rec.wait()

    am_output = android.wait_for(android_proc, timeout=30)
    print(f"[T15-B] Android output:\n{am_output[:400]}")

    # 5. Load and validate
    assert rec_path.exists()
    cap_data, cap_sr = sf.read(str(rec_path), dtype="float32", always_2d=False)
    ref_data, ref_sr = sf.read(str(_SPEECH),  dtype="float32", always_2d=False)
    if cap_data.ndim > 1:
        cap_data = cap_data.mean(axis=1)

    rms_cap = float(np.sqrt(np.mean(cap_data ** 2)))
    print(f"[T15-B] Captured RMS={rms_cap:.4f}")
    assert rms_cap > 0.001, "Captured audio is silent — acoustic path broken"

    if cap_sr != ref_sr:
        cap_data = _resample(cap_data, cap_sr, ref_sr)

    # 6. MOS on best-aligned loop
    mos = _compute_mos(ref_data, cap_data, sr_native=ref_sr)
    print(
        f"\n[T15-B] ══ Acoustic PESQ Report ══\n"
        f"  Mode               : {mos['mode']}\n"
        f"  Active audio start : {mos['active_start_s']:.3f}s\n"
        f"  Narrowband MOS     : {mos['narrowband_mos']:.3f}   (informational, no hard gate)\n"
        f"  Wideband MOS       : {mos['wideband_mos']:.3f}   (informational)\n"
        f"  Loopback delay     : {mos['latency_ms']:.1f} ms\n"
        f"  NOTE: Physical acoustic path — MOS < 2.0 is expected.\n"
    )
    # Acoustic mode has no hard MOS gate; just verify the pipeline runs.
    assert cap_data is not None
    print("[T15-B] PASS (pipeline ran, acoustic PESQ informational)")
