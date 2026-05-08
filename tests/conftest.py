"""
conftest.py - Pytest session fixtures for anyMic audio test tools.

Provides:
  fixture_chirp  -> Path to tests/fixtures/chirp.wav
  fixture_sweep  -> Path to tests/fixtures/sweep.wav
  fixture_speech -> Path to tests/fixtures/speech.wav

If the WAV files are not present they are generated automatically
by the corresponding gen_*.py scripts.
"""

import subprocess
import sys
from pathlib import Path

import pytest

# Locate the tests/ directory (this file lives in it)
_TESTS_DIR   = Path(__file__).parent.resolve()
_FIXTURES    = _TESTS_DIR / "fixtures"
_TOOLS       = _TESTS_DIR / "tools"
_PYTHON      = sys.executable


def _ensure_wav(wav_path: Path, gen_script: Path, extra_args: list[str] | None = None) -> Path:
    """Generate wav_path if it does not already exist."""
    if wav_path.exists():
        return wav_path
    _FIXTURES.mkdir(parents=True, exist_ok=True)
    cmd = [_PYTHON, str(gen_script), "--out", str(wav_path)]
    if extra_args:
        cmd.extend(extra_args)
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"Failed to generate {wav_path}:\n"
            f"  stdout: {result.stdout}\n"
            f"  stderr: {result.stderr}"
        )
    return wav_path


@pytest.fixture(scope="session")
def fixture_chirp() -> Path:
    """Path to the chirp fixture WAV (generated if missing)."""
    return _ensure_wav(
        _FIXTURES / "chirp.wav",
        _TOOLS / "gen_chirp.py",
    )


@pytest.fixture(scope="session")
def fixture_sweep() -> Path:
    """Path to the sweep fixture WAV (generated if missing)."""
    return _ensure_wav(
        _FIXTURES / "sweep.wav",
        _TOOLS / "gen_sweep.py",
    )


@pytest.fixture(scope="session")
def fixture_speech() -> Path:
    """Path to the speech fixture WAV (generated if missing)."""
    return _ensure_wav(
        _FIXTURES / "speech.wav",
        _TOOLS / "gen_speech.py",
    )
