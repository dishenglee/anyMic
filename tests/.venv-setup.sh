#!/usr/bin/env bash
# .venv-setup.sh — Create and populate the tests/.venv for anyMic T14 tests.
#
# Usage:
#   bash tests/.venv-setup.sh
#
# The venv is placed at tests/.venv/ relative to the anyMic root.
# If the venv already exists, only a pip upgrade/install is run.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV_DIR="${SCRIPT_DIR}/.venv"
REQ_FILE="${SCRIPT_DIR}/requirements.txt"

echo "[venv-setup] Script dir : ${SCRIPT_DIR}"
echo "[venv-setup] Venv dir   : ${VENV_DIR}"
echo "[venv-setup] Requirements: ${REQ_FILE}"

# ── Pick Python 3.10+ ─────────────────────────────────────────────────────────
if command -v python3.13 &>/dev/null; then
    PYTHON=python3.13
elif command -v python3.12 &>/dev/null; then
    PYTHON=python3.12
elif command -v python3.11 &>/dev/null; then
    PYTHON=python3.11
elif command -v python3.10 &>/dev/null; then
    PYTHON=python3.10
elif command -v python3 &>/dev/null; then
    PYTHON=python3
else
    echo "[venv-setup] ERROR: No python3 found on PATH" >&2
    exit 1
fi

PYTHON_VERSION=$("${PYTHON}" --version 2>&1)
echo "[venv-setup] Using ${PYTHON} (${PYTHON_VERSION})"

# ── Create venv if needed ─────────────────────────────────────────────────────
if [ ! -d "${VENV_DIR}" ]; then
    echo "[venv-setup] Creating virtual environment…"
    "${PYTHON}" -m venv "${VENV_DIR}"
else
    echo "[venv-setup] Virtual environment already exists, skipping creation."
fi

# ── Upgrade pip and install requirements ──────────────────────────────────────
echo "[venv-setup] Upgrading pip…"
"${VENV_DIR}/bin/pip" install --quiet --upgrade pip

echo "[venv-setup] Installing requirements from ${REQ_FILE}…"
"${VENV_DIR}/bin/pip" install --quiet -r "${REQ_FILE}"

echo "[venv-setup] Done. Activate with: source ${VENV_DIR}/bin/activate"
echo "[venv-setup] Or run directly  : ${VENV_DIR}/bin/python3"
