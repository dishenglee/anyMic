"""
conftest.py — pytest fixtures for anyMic end-to-end integration tests (T14).

Session-scoped fixtures:
  mac_server      — running MacServer instance
  android_client  — AndroidClient (APK already installed)
  blackhole_idx   — detected BlackHole AVFoundation device index
"""

from __future__ import annotations

import pytest
from pathlib import Path

from .orchestrator import MacServer, AndroidClient, BlackHoleRecorder, get_local_ip


# ---------------------------------------------------------------------------
# mac_server fixture
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def mac_server():
    """Start the anyMic Tauri server and yield; stop on teardown."""
    s = MacServer(release=True)
    s.start()
    try:
        s.wait_ready(timeout=90)
    except RuntimeError as exc:
        pytest.fail(f"Mac server failed to start: {exc}\n\nLog:\n{s.log_tail()}")
    yield s
    s.stop()


# ---------------------------------------------------------------------------
# android_client fixture
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def android_client():
    """Build + install the debug test APK; yield the client instance."""
    c = AndroidClient()
    c.install_apk()
    yield c
    c.force_stop()


# ---------------------------------------------------------------------------
# local_ip fixture
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def local_ip():
    """Return the Mac's LAN IPv4 address that the Android device can reach."""
    return get_local_ip()


# ---------------------------------------------------------------------------
# blackhole_idx fixture
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def blackhole_idx():
    """Return the detected BlackHole 2ch AVFoundation device index."""
    return BlackHoleRecorder._detect_blackhole_index()
