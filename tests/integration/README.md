# anyMic T14 — End-to-End Latency Tests

Automated end-to-end latency measurement for the full acoustic loop:

```
Mac speaker (afplay chirp)
  → Android microphone
  → Opus / UDP
  → Mac anymic-app server
  → BlackHole 2ch (virtual audio device)
  → ffmpeg recording
  → Python xcorr_latency
  → P50 / P95 latency numbers
```

---

## Prerequisites

### Mac host

| Item | Details |
|------|---------|
| macOS 12+ | Same Wi-Fi network as the Android device |
| Homebrew | `brew install ffmpeg` |
| ffmpeg 8+ | `ffmpeg -version` |
| BlackHole 2ch | [existential.audio/blackhole](https://existential.audio/blackhole/) |
| BlackHole routing | System Settings → Sound → Output → **BlackHole 2ch** |
| Rust + Cargo | `rustup toolchain install stable` |
| Python 3.10+ | `python3 --version` |

### Android device

| Item | Details |
|------|---------|
| Android 8+ (API 26+) | Xiaomi M2012K11AC (Android 13) confirmed |
| USB debugging | Enabled in Developer Options |
| adb | `brew install android-platform-tools` |
| RECORD_AUDIO | Granted (instrumented test grants via shell permission) |
| Same Wi-Fi | Device must reach Mac on port 50127 (UDP) + 50128 (TCP) |

### Android SDK + NDK

```bash
# Android SDK via command-line tools or Android Studio
export ANDROID_HOME=~/Library/Android/sdk
export PATH=$PATH:$ANDROID_HOME/platform-tools:$ANDROID_HOME/build-tools/34.0.0

# NDK r26+ for Opus JNI build
sdkmanager "ndk;26.1.10909125"
```

---

## Quick Start

```bash
cd /Users/libaiwan/Documents/anyMic

# 1. Set up the Python venv
bash tests/.venv-setup.sh

# 2. Connect Android device via USB, confirm adb sees it
adb devices

# 3. Run all E2E tests
make test-e2e
# — or —
tests/.venv/bin/pytest tests/integration -v -s --tb=short
```

---

## Running Individual Tests

```bash
# Just smoke tests (no server / Android needed)
tests/.venv/bin/pytest tests/integration::test_blackhole_detected -v

# Single-shot latency test
tests/.venv/bin/pytest tests/integration::test_e2e_latency_single -v -s

# Full P50/P95 multi-chirp test
tests/.venv/bin/pytest tests/integration::test_e2e_latency_p50_p95 -v -s
```

---

## How It Works

### Python orchestrator (`orchestrator.py`)

| Class | Responsibility |
|-------|---------------|
| `MacServer` | `cargo run -p anymic-app --release` in background; watches for `"server started"` in log |
| `AndroidClient` | `./gradlew assembleDebug assembleDebugAndroidTest` + `adb install` + `am instrument` |
| `BlackHoleRecorder` | Detects BlackHole AVFoundation index; records with `ffmpeg -f avfoundation` |
| `SignalPlayer` | Plays chirp through Mac speakers with `afplay` (pre-installed, no deps) |
| `get_local_ip()` | `ipconfig getifaddr en0/en1`; UDP socket fallback |

### Android side (`RemoteConnectTest.kt`)

Triggered via `adb shell am instrument`. Directly wires:
```
ControlChannel → handshake with real server
UdpSender → sends Opus frames to server UDP port
AudioCapture → 48 kHz mono PCM via AudioRecord
OpusEncoder → real Opus encoding
```

**No mDNS used**: IP/ports passed as `-e host -e dataPort -e controlPort` arguments.  
**No main/ code changes**: test-only file in `androidTest/`, reuses existing `net/audio` modules.

### Signal chain timing

```
t=0     BlackHole recording starts (ffmpeg)
t=1s    Android am instrument starts
t=4s    (warmup) chirp plays via afplay
t=4+Δ   Android mic captures chirp
t=4+Δ+encode  Opus frames sent over UDP
t=4+Δ+encode+jitter  Server decodes → BlackHole
t=end   ffmpeg stops, xcorr_latency measures Δ = end-to-end latency
```

---

## Makefile

```bash
make test-e2e      # full end-to-end suite
make test-server   # Rust server unit tests only
make test-android  # Android instrumented tests (includes loopback E2E)
```

---

## Expected Output

```
[E2E] Mac IP = 192.168.68.8
[E2E] BlackHole index = 0
[E2E] Recording → /tmp/pytest-.../recorded_single.wav
[E2E] Android instrumented test started (async)
[E2E] Playing chirp through Mac speaker…
[E2E] Recording size = 847.3 KB

[T14] SINGLE-SHOT LATENCY = 73.2 ms  (peak_corr=0.847, snr=24.3 dB)

[P50/P95] Chirp 1/5: latency=71.4 ms, peak_corr=0.831
[P50/P95] Chirp 2/5: latency=74.8 ms, peak_corr=0.862
...

[T14] MULTI-CHIRP LATENCY REPORT
  N valid measurements : 5/5
  All latencies (ms)   : ['71.4', '74.8', '73.2', '72.9', '75.1']
  P50                  : 73.2 ms
  P95 (max for N<20)   : 75.1 ms
  Target P95 < 80 ms   : PASS
```

---

## Troubleshooting

### No chirp peak detected (`peak_correlation < 0.3`)

1. **System audio output**: System Settings → Sound → Output → confirm **BlackHole 2ch** is selected
2. **Volume**: Mac speaker volume must be audible; afplay uses system volume
3. **Android not connected**: Check `am instrument` output for handshake errors
4. **Wrong BlackHole index**: `ffmpeg -f avfoundation -list_devices true -i ""` shows current indices
5. **Bluetooth output device**: If your system default output is a Bluetooth device (e.g. 小爱音箱),
   the chirp signal will be degraded by Bluetooth codec compression.
   `SignalPlayer` automatically switches to the Mac built-in speaker during chirp playback and
   restores the original device afterwards.  This requires `SwitchAudioSource`:
   ```bash
   brew install switchaudio-osx
   SwitchAudioSource -a   # confirm "MacBook Pro扬声器" (or similar) is listed
   ```
   After each test run the system output is restored to the original device.
   To verify manually: `SwitchAudioSource -c` should show your original device.

### Android test fails to start

```bash
adb devices          # must show "device" not "unauthorized"
adb shell whoami     # confirms shell access
```

MIUI/Xiaomi devices require:
```bash
# Grant RECORD_AUDIO via shell (requires USB debugging)
adb shell am instrument ... # SafeGrantPermissionRule handles this automatically
# If still failing: Settings → App info → anyMic → Permissions → Microphone → Allow
```

### Android can't reach Mac server

1. **Same Wi-Fi subnet**: `adb shell ip route` should show same /24 as Mac
2. **Mac firewall**: System Settings → Network → Firewall → allow anymic-app (or allow ports 50127/50128)
3. **MIUI aggressive Wi-Fi**: Developer Options → Wireless → disable Wi-Fi optimization
4. **Test TCP connectivity**:
   ```bash
   adb shell nc -z 192.168.68.8 50128 && echo "TCP OK"
   adb shell nc -zu 192.168.68.8 50127 && echo "UDP OK"
   ```

### Mac server won't start

```bash
# Check BlackHole is installed
ffmpeg -f avfoundation -list_devices true -i "" 2>&1 | grep -i black

# Run server manually
cd server && RUST_LOG=info cargo run -p anymic-app --release
```

### ffmpeg can't open BlackHole

BlackHole index may change after a system reboot.  The orchestrator auto-detects it.
If it fails, find the index manually:
```bash
ffmpeg -f avfoundation -list_devices true -i "" 2>&1
# Look for: [N] BlackHole 2ch
```

### MIUI multicast issues (mDNS)

This test suite bypasses mDNS entirely — we use the Mac's LAN IP directly.
mDNS (T07) may fail on MIUI due to multicast blocking; this test is immune.

---

## Known Issues

| Issue | Impact | Workaround |
|-------|--------|-----------|
| MIUI blocks multicast | mDNS discovery fails | T14 uses direct IP (bypassed) |
| Mac firewall drops UDP | No audio received | Allow anymic-app in firewall |
| BlackHole not set as output | chirp not captured | Set BlackHole as system output |
| MIUI mic permission | AudioCapture returns 0 | Grant manually in Settings |
| Server TCP control stub | HelloAck not sent | Test streams UDP directly, still measures latency |

---

## Performance Target

| Metric | T14 Gate | M5 Goal |
|--------|---------|---------|
| `peak_correlation` | > 0.3 | > 0.7 |
| P50 latency | < 300 ms | < 60 ms |
| P95 latency | measured | < 80 ms |

The T14 gate is conservative — the test passes as long as the acoustic loop works.
P95 < 80 ms is the M5 performance milestone, tracked separately.

---

## T15 — Quality Tests

PESQ (ITU-T P.862) and frequency-domain quality tests for the anyMic Opus loopback path.

### Test files

| File | Description |
|------|-------------|
| `tests/integration/test_pesq.py` | PESQ MOS-LQO narrowband + wideband (Mode A: server loopback, Mode B: acoustic) |
| `tests/integration/test_spectrum.py` | SNR / THD / frequency response deviation via `analyze_spectrum.py` |

### Running T15

```bash
# Mode A (server-only loopback, no Android needed) — runs automatically
tests/.venv/bin/pytest tests/integration/test_pesq.py tests/integration/test_spectrum.py -v -s

# Or via make (if Makefile has the target):
make test-quality
```

### Mode A — Server-only loopback (default, always runs)

The test starts the anyMic Tauri server, sends `speech.wav` (or `sweep.wav`)
through `send_opus_stream.py` over UDP to the local server, records the
decoded output from BlackHole with ffmpeg, then computes quality metrics.

No Android device is required.

### Mode B — Full acoustic path (optional)

```bash
ANYMIC_ACOUSTIC=1 tests/.venv/bin/pytest tests/integration/test_pesq.py -v -s -k acoustic
```

Requires:
- Android device connected via USB
- Mac and Android on the same Wi-Fi network
- Physical speaker → mic acoustic path

Mode B has no hard MOS gate; the full acoustic chain introduces distortion
(speaker → air → mic → AGC → Opus 32kbps) that typically yields PESQ < 2.0.

### Quality thresholds

| Test | Metric | Threshold | Notes |
|------|--------|-----------|-------|
| PESQ Mode A | Narrowband MOS | > 2.0 | Server-only Opus loopback, near-lossless |
| PESQ Mode A | Wideband MOS | > 1.5 | 32 kbps VOIP trades off MOS vs bandwidth |
| Spectrum | SNR in-band | > 15 dB | Primary 80–8000 Hz, fallback 200–4000 Hz |
| Spectrum | THD | < 10 % | Opus VOIP frame tolerance |
| Spectrum | Freq response max dev | < 6 dB | Per-band energy ratio deviation |

### Known behaviour

- **Opus 32 kbps VOIP** is optimised for speech intelligibility, not MOS maximisation.
  Best-case PESQ MOS on a loopback path at this bitrate is approximately **3.5 (wideband)**
  for clean narrowband speech; real-world scores with jitter/packet-loss margin are lower.
- The narrowband MOS > 2.0 gate is deliberately conservative so server-side regressions
  (muted output, wrong codec, pipeline broken) are caught reliably.
- **Sweep SNR**: Opus VOIP is not optimised for wideband sine sweeps. If the primary
  80–8000 Hz band SNR is below 10 dB, the spectrum test automatically falls back to
  the 200–4000 Hz speech-core band and reports which band was used.
- **pesq Python package** requires a local C extension build. On macOS arm64 this
  succeeds without issues. If `pip install pesq` fails, the test falls back to `pystoi`
  (STOI intelligibility score, not MOS) and reports `stoi_score` alongside a
  linearly-mapped pseudo-MOS value.

### Dependency

```
pesq>=0.0.4   # ITU-T P.862 reference implementation (C extension, MIT)
```

Install:

```bash
tests/.venv/bin/pip install -r tests/requirements.txt
```
