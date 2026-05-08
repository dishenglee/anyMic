"""
orchestrator.py — Manage mac server + Android client + ffmpeg recording
for anyMic end-to-end latency tests (T14).

Components:
  MacServer        — launch anymic-app (Tauri) server in background
  AndroidClient    — build/install/trigger instrumented test via adb
  BlackHoleRecorder— capture from BlackHole 2ch with ffmpeg
  SignalPlayer     — play a WAV through Mac default output (afplay)
  get_local_ip()   — discover the LAN IPv4 address reachable from Android
"""

from __future__ import annotations

import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path


# ---------------------------------------------------------------------------
# Project root (two levels up from this file)
# ---------------------------------------------------------------------------
_HERE = Path(__file__).parent.resolve()
_ROOT = _HERE.parent.parent  # anyMic/


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    """Run a command, raise on non-zero exit."""
    return subprocess.run(cmd, check=True, **kw)


def get_local_ip(ifaces: list[str] | None = None) -> str:
    """
    Return the LAN IPv4 address visible to the Android device on the same Wi-Fi.

    Strategy (in order):
      1. Try `ipconfig getifaddr <iface>` for en0, en1, en2 …
      2. Fall back to connecting a UDP socket to 8.8.8.8 and reading the local address.

    Filters out loopback (127.x) and link-local (169.254.x).
    """
    if ifaces is None:
        ifaces = ["en0", "en1", "en2", "en3", "utun0"]

    for iface in ifaces:
        try:
            result = subprocess.run(
                ["ipconfig", "getifaddr", iface],
                capture_output=True, text=True, timeout=2
            )
            ip = result.stdout.strip()
            if ip and not ip.startswith("127.") and not ip.startswith("169.254."):
                return ip
        except (FileNotFoundError, subprocess.TimeoutExpired):
            continue

    # UDP socket fallback
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("8.8.8.8", 80))
            ip = s.getsockname()[0]
        if ip and not ip.startswith("127."):
            return ip
    except OSError:
        pass

    raise RuntimeError(
        "Cannot determine local LAN IP. "
        "Make sure Wi-Fi is connected and the device is on the same network."
    )


# ---------------------------------------------------------------------------
# MacServer
# ---------------------------------------------------------------------------

class MacServer:
    """
    Start the anymic Tauri app (which auto-starts the server) in the background.

    The server logs to stderr in JSON/structured form.  We watch for either:
      - "server started" — the structured log message emitted by start_server()
      - "auto-started server" — the Tauri main.rs message
    """

    READY_PATTERNS = [
        b"server started",
        b"auto-started server",
        b"Listening UDP",
    ]

    def __init__(
        self,
        cargo_manifest: Path | None = None,
        release: bool = True,
        data_port: int = 50127,
        control_port: int = 50128,
    ):
        self.cargo_manifest = cargo_manifest or (_ROOT / "server")
        self.release = release
        self.data_port = data_port
        self.control_port = control_port
        self._proc: subprocess.Popen | None = None
        self._log_lines: list[str] = []
        self._ready = threading.Event()
        self._log_thread: threading.Thread | None = None

    def start(self) -> None:
        """Build (if needed) and launch the server process."""
        cmd = ["cargo", "run", "-p", "anymic-app"]
        if self.release:
            cmd.append("--release")

        env = os.environ.copy()
        env["RUST_LOG"] = "info"
        # Disable the Tauri window to run headless (if supported)
        env.setdefault("TAURI_DISABLE_WINDOW", "1")

        self._proc = subprocess.Popen(
            cmd,
            cwd=str(self.cargo_manifest),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,  # merge both streams
            env=env,
        )
        self._log_thread = threading.Thread(
            target=self._tail_log, daemon=True
        )
        self._log_thread.start()

    def _tail_log(self) -> None:
        """Read server output in a background thread; signal when ready."""
        assert self._proc is not None
        assert self._proc.stdout is not None
        for raw in self._proc.stdout:
            line = raw.decode(errors="replace").rstrip()
            self._log_lines.append(line)
            for pat in self.READY_PATTERNS:
                if pat in raw:
                    self._ready.set()

    def wait_ready(self, timeout: float = 60.0) -> None:
        """
        Block until the server is ready or *timeout* seconds elapse.
        Raises RuntimeError if the server doesn't become ready in time.
        """
        if self._ready.wait(timeout):
            return
        # Dump last 20 lines for diagnosis
        tail = "\n".join(self._log_lines[-20:])
        raise RuntimeError(
            f"Mac server did not become ready within {timeout}s.\n"
            f"Last log lines:\n{tail}"
        )

    def is_running(self) -> bool:
        return self._proc is not None and self._proc.poll() is None

    def stop(self) -> None:
        """Gracefully stop the server (SIGINT → 5s → SIGKILL)."""
        if self._proc is None:
            return
        proc = self._proc
        if proc.poll() is not None:
            return  # already dead

        try:
            proc.send_signal(signal.SIGINT)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=3)
        except ProcessLookupError:
            pass  # already gone

        self._proc = None

    def log_tail(self, n: int = 30) -> str:
        return "\n".join(self._log_lines[-n:])


# ---------------------------------------------------------------------------
# AndroidClient
# ---------------------------------------------------------------------------

class AndroidClient:
    """
    Build the debug-test APK, install it, and trigger RemoteConnectTest
    via `adb shell am instrument`.
    """

    TEST_PACKAGE = "com.anymic.app.test"
    TEST_RUNNER = "androidx.test.runner.AndroidJUnitRunner"
    TEST_CLASS = "com.anymic.app.net.RemoteConnectTest"

    def __init__(
        self,
        device_id: str = "9657ea7e",
        android_root: Path | None = None,
    ):
        self.device_id = device_id
        self.android_root = android_root or (_ROOT / "android")
        self._am_proc: subprocess.Popen | None = None

    def _adb(self, *args: str) -> list[str]:
        return ["adb", "-s", self.device_id, *args]

    def install_apk(self) -> None:
        """Build the Android test APK and install it on the device."""
        print("[AndroidClient] Building debug test APK…")
        _run(
            ["./gradlew", ":app:assembleDebug", ":app:assembleDebugAndroidTest"],
            cwd=str(self.android_root),
            capture_output=False,
        )
        apk_paths = list(
            (self.android_root / "app" / "build" / "outputs" / "apk").rglob("*.apk")
        )
        # Install main APK first, then test APK
        for apk in sorted(apk_paths):
            print(f"[AndroidClient] Installing {apk.name}…")
            _run(self._adb("install", "-r", "-t", str(apk)))

    def connect(
        self,
        host: str,
        data_port: int = 50127,
        control_port: int = 50128,
        duration_ms: int = 12_000,
    ) -> subprocess.CompletedProcess:
        """
        Synchronously run RemoteConnectTest via am instrument.
        Blocks until the test completes or times out.
        """
        cmd = self._adb(
            "shell", "am", "instrument", "-w",
            "-e", "class", self.TEST_CLASS,
            "-e", "host", host,
            "-e", "dataPort", str(data_port),
            "-e", "controlPort", str(control_port),
            "-e", "durationMs", str(duration_ms),
            f"{self.TEST_PACKAGE}/{self.TEST_RUNNER}",
        )
        timeout_s = duration_ms / 1000.0 + 60  # generous headroom
        return subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )

    def connect_async(
        self,
        host: str,
        data_port: int = 50127,
        control_port: int = 50128,
        duration_ms: int = 12_000,
    ) -> subprocess.Popen:
        """
        Asynchronously run RemoteConnectTest; return the Popen handle.
        Call wait_for() to join it.
        """
        cmd = self._adb(
            "shell", "am", "instrument", "-w",
            "-e", "class", self.TEST_CLASS,
            "-e", "host", host,
            "-e", "dataPort", str(data_port),
            "-e", "controlPort", str(control_port),
            "-e", "durationMs", str(duration_ms),
            f"{self.TEST_PACKAGE}/{self.TEST_RUNNER}",
        )
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self._am_proc = proc
        return proc

    def wait_for(self, proc: subprocess.Popen, timeout: float = 60.0) -> str:
        """Wait for an async am instrument run; return its combined output."""
        try:
            stdout, stderr = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            stdout, stderr = proc.communicate()
        return (stdout or "") + (stderr or "")

    def force_stop(self) -> None:
        """Kill the app on device."""
        subprocess.run(
            self._adb("shell", "am", "force-stop", "com.anymic.app"),
            capture_output=True,
        )
        subprocess.run(
            self._adb("shell", "am", "force-stop", self.TEST_PACKAGE),
            capture_output=True,
        )


# ---------------------------------------------------------------------------
# BlackHoleRecorder
# ---------------------------------------------------------------------------

class BlackHoleRecorder:
    """
    Record from the BlackHole 2ch virtual device using ffmpeg's avfoundation input.

    The BlackHole audio device index in the AVFoundation list is detected
    automatically at construction time.
    """

    def __init__(self, device_index: int | None = None):
        if device_index is not None:
            self.device_index = device_index
        else:
            self.device_index = self._detect_blackhole_index()
        self._proc: subprocess.Popen | None = None

    @staticmethod
    def _detect_blackhole_index() -> int:
        """Query ffmpeg for the AVFoundation audio device list and return BlackHole's index."""
        result = subprocess.run(
            ["ffmpeg", "-f", "avfoundation", "-list_devices", "true", "-i", ""],
            capture_output=True, text=True, timeout=10
        )
        combined = result.stdout + result.stderr
        # Look for lines like: [0] BlackHole 2ch
        for line in combined.splitlines():
            m = re.search(r"\[(\d+)\]\s+BlackHole", line, re.IGNORECASE)
            if m:
                return int(m.group(1))
        raise RuntimeError(
            "BlackHole 2ch not found in AVFoundation device list.\n"
            "Install BlackHole from https://existential.audio/blackhole/\n"
            f"ffmpeg output:\n{combined}"
        )

    def start(self, output_path: str, duration_s: float = 15.0) -> None:
        """
        Start recording from BlackHole to *output_path*.
        Recording stops automatically after *duration_s* seconds.
        Non-blocking: use wait() to join.
        """
        # avfoundation audio-only: -i ":<audio_index>"
        cmd = [
            "ffmpeg", "-y",
            "-f", "avfoundation",
            "-i", f":{self.device_index}",
            "-t", str(duration_s),
            "-ar", "48000",
            "-ac", "1",
            str(output_path),
        ]
        self._proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def wait(self) -> None:
        """Block until the ffmpeg recording finishes."""
        if self._proc is not None:
            self._proc.wait()
            self._proc = None

    def stop_early(self) -> None:
        """Interrupt recording before the duration elapses."""
        if self._proc and self._proc.poll() is None:
            self._proc.send_signal(signal.SIGINT)
            try:
                self._proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._proc.kill()
            self._proc = None


# ---------------------------------------------------------------------------
# SignalPlayer
# ---------------------------------------------------------------------------

# Candidate names for the Mac built-in speaker, tried in order.
_BUILTIN_SPEAKER_CANDIDATES = [
    "MacBook Pro扬声器",
    "MacBook Air扬声器",
    "MacBook Pro Speakers",
    "MacBook Air Speakers",
    "Built-in Output",
    "Built-in Speakers",
    "Internal Speakers",
]


def _get_switchaudiosource() -> str | None:
    """Return the path to SwitchAudioSource, or None if not installed."""
    return shutil.which("SwitchAudioSource")


def _list_audio_devices() -> list[str]:
    """Return all audio output device names via SwitchAudioSource -a."""
    sas = _get_switchaudiosource()
    if sas is None:
        return []
    try:
        out = subprocess.check_output([sas, "-a"], text=True, timeout=5)
        return [line.strip() for line in out.splitlines() if line.strip()]
    except Exception:
        return []


def _find_builtin_speaker(devices: list[str]) -> str | None:
    """
    Return the device name of the Mac built-in speaker from *devices*.

    Tries exact matches from _BUILTIN_SPEAKER_CANDIDATES first, then falls back
    to a substring search for "MacBook", "Built-in", or "Internal".
    """
    # Exact match from known candidate names
    for candidate in _BUILTIN_SPEAKER_CANDIDATES:
        if candidate in devices:
            return candidate
    # Fuzzy fallback
    keywords = ("MacBook", "Built-in", "Internal")
    for device in devices:
        for kw in keywords:
            if kw.lower() in device.lower():
                return device
    return None


class SignalPlayer:
    """
    Play a WAV file through the Mac's default audio output (speakers) using afplay.

    afplay is used (over ffplay) because:
      - Pre-installed on macOS (no dependency)
      - Routes through the system audio graph → same latency path as the app
      - Simple blocking invocation with exact playback (no decode overhead visible to timing)

    When *force_builtin_speaker* is True (the default), SignalPlayer temporarily
    switches the system default output to the Mac built-in speaker for the duration
    of playback, then restores the original device.  This avoids Bluetooth codec
    distortion (e.g. from a 小爱音箱) that degrades the chirp's cross-correlation.

    Requires SwitchAudioSource (brew install switchaudio-osx).  If it is not
    installed the flag is silently ignored and afplay uses whatever device is
    currently active.
    """

    def __init__(self, force_builtin_speaker: bool = True):
        self.force_builtin_speaker = force_builtin_speaker

    def _switch_to_builtin(self) -> str | None:
        """
        Switch system output to the Mac built-in speaker.

        Returns the name of the previous default device so the caller can
        restore it, or None if the switch was not possible.
        """
        sas = _get_switchaudiosource()
        if sas is None:
            print("[SignalPlayer] SwitchAudioSource not found — skipping device switch")
            return None

        devices = _list_audio_devices()
        builtin = _find_builtin_speaker(devices)
        if builtin is None:
            print(
                "[SignalPlayer] Could not identify built-in speaker from device list: "
                f"{devices} — skipping device switch"
            )
            return None

        # Save current device
        try:
            prev = subprocess.check_output([sas, "-c"], text=True, timeout=5).strip()
        except Exception as exc:
            print(f"[SignalPlayer] Could not query current device: {exc}")
            return None

        if prev == builtin:
            # Already on built-in speaker; nothing to do, but return prev so
            # the finally block still calls restore (which is a no-op).
            print(f"[SignalPlayer] Already on built-in speaker '{builtin}' — no switch needed")
            return prev

        try:
            subprocess.check_call([sas, "-s", builtin], timeout=5)
            print(f"[SignalPlayer] Switched audio output: '{prev}' → '{builtin}'")
        except Exception as exc:
            print(f"[SignalPlayer] Failed to switch to '{builtin}': {exc}")
            return None

        return prev

    def _restore_device(self, prev: str | None) -> None:
        """Restore the system output to *prev* (the device saved before the switch)."""
        if prev is None:
            return
        sas = _get_switchaudiosource()
        if sas is None:
            return
        try:
            # Only bother if we're not already on the right device
            current = subprocess.check_output([sas, "-c"], text=True, timeout=5).strip()
            if current == prev:
                return
            subprocess.check_call([sas, "-s", prev], timeout=5)
            print(f"[SignalPlayer] Restored audio output: '{current}' → '{prev}'")
        except Exception as exc:
            print(f"[SignalPlayer] WARNING: Failed to restore audio device to '{prev}': {exc}")

    def play(self, wav_path: str, blocking: bool = True) -> subprocess.Popen | None:
        """
        Play *wav_path* with afplay.

        If *force_builtin_speaker* was set at construction, temporarily switches
        the system default output to the Mac built-in speaker for the duration of
        this call, then restores the original device.

        If blocking=True, waits for playback to finish.
        If blocking=False, returns the Popen handle (device is restored after
        the process completes only when blocking=True; for non-blocking callers
        the device switch is still performed but restoration must be handled
        by the caller via _restore_device if needed).
        """
        prev_device: str | None = None
        if self.force_builtin_speaker:
            prev_device = self._switch_to_builtin()

        cmd = ["afplay", str(wav_path)]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        if blocking:
            try:
                proc.wait()
            finally:
                self._restore_device(prev_device)
            return None

        # Non-blocking: restore device after process finishes in a background thread
        if prev_device is not None:
            def _restore_after(p: subprocess.Popen, prev: str) -> None:
                p.wait()
                self._restore_device(prev)

            t = threading.Thread(target=_restore_after, args=(proc, prev_device), daemon=True)
            t.start()

        return proc
