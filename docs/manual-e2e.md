# anyMic Manual End-to-End Verification Guide

**Status:** T11 baseline  
**Date:** 2026-05-08  
**Purpose:** Human-readable steps for verifying full-stack operation (Mac server + Android client).
This document is the baseline for T14 automated end-to-end tests.

---

## Prerequisites

| Item | Requirement |
|------|-------------|
| macOS host | same Wi-Fi network as Android device |
| Android device | minSdk 26 (Android 8), RECORD_AUDIO granted |
| adb | on PATH (`brew install android-platform-tools`) |
| ffmpeg | optional, for audio capture verification |
| BlackHole 2ch | optional, virtual audio device for loopback verification |

---

## 1. Start the Mac Server

```bash
# Build and run the Rust/Tauri server
cd /Users/libaiwan/Documents/anyMic/server
cargo run --release
```

The server starts mDNS advertisement automatically on startup.  
Verify via the system log that it prints:

```
Listening UDP 0.0.0.0:50127
Listening TCP 0.0.0.0:50128
mDNS registered: _anymic._udp.local. (v=1 codec=opus48 fid=<8hex>)
```

---

## 2. Build the Debug APK

```bash
cd /Users/libaiwan/Documents/anyMic/android
./gradlew :app:assembleDebug
```

Output APK: `android/app/build/outputs/apk/debug/app-debug.apk`

---

## 3. Install and Grant Permissions

```bash
# Connect device via USB with USB debugging enabled
adb devices           # confirm device shows as "device"

# Install APK
adb install -r android/app/build/outputs/apk/debug/app-debug.apk

# Grant RECORD_AUDIO (on MIUI/Xiaomi, requires root or manual grant)
# Option A: via root shell
adb shell su -c "pm grant com.anymic.app android.permission.RECORD_AUDIO"

# Option B: on-device — System Settings → App Info → anyMic → Permissions → Microphone → Allow
```

---

## 4. Device Operation Steps

1. Open the **anyMic** app on the Android device.
2. Ensure the device is on the **same Wi-Fi network** as the Mac server.
3. Tap **"Discover"** — the app transitions to `Discovering` state and begins mDNS browsing.
4. Wait up to 5 seconds. The discovered server (e.g., `MacBook Pro (M3)`) appears in the list.
5. Tap the server name — state transitions to `Connecting`, then `Streaming`.
6. The streaming screen shows:
   - `pkts` — rising packet count (200/s at 5ms frames)
   - `rtt` — round-trip time in ms
   - `src` — audio source (`VOICE_RECOGNITION` / `UNPROCESSED` / `MIC`)
   - `session` — first 8 chars of session UUID

---

## 5. Audio Verification

### Option A — System Audio (no extra tools)

- Mac: open **System Preferences → Sound → Input** and watch the input level meter.  
  While the Android device is streaming, the meter should show activity corresponding to sounds near the phone microphone.

### Option B — QuickTime screen capture

```bash
# Start QuickTime Player → File → New Audio Recording
# Select "anyMic" or the system microphone in the input dropdown
# Click Record, speak near the Android phone, stop recording, inspect waveform
```

### Option C — BlackHole + ffmpeg (most rigorous)

```bash
# Prerequisite: install BlackHole 2ch (https://existential.audio/blackhole/)
# In System Audio MIDI Setup, set BlackHole as the Mac microphone input destination.
# The server routes the incoming UDP stream to the system audio output device.

# Record 10 seconds from BlackHole into a WAV file:
ffmpeg -f avfoundation -i ":BlackHole 2ch" -t 10 /tmp/anymic_verify.wav

# Analyze the result:
ffprobe -v quiet -show_streams /tmp/anymic_verify.wav | grep -E "sample_rate|channels|duration"
# Expected: sample_rate=48000, channels=1, duration≈10.0

# Check for non-silence (the RMS level should be above -60 dBFS when speaking):
ffmpeg -i /tmp/anymic_verify.wav -af astats -f null - 2>&1 | grep "RMS level"
```

---

## 6. Automated Instrumented Test Suite

These tests run entirely on-device without a real Mac server:

```bash
cd /Users/libaiwan/Documents/anyMic/android

# Build both APKs
./gradlew :app:assembleDebug :app:assembleDebugAndroidTest

# Run all instrumented tests (device must be connected via adb)
./gradlew :app:connectedDebugAndroidTest

# Results: android/app/build/reports/androidTests/connected/debug/index.html
```

### Test inventory (17 tests, all must pass)

| Class | Test | What it checks |
|-------|------|---------------|
| `AudioCaptureTest` | `captures_about_2000_frames_in_10_seconds` | 10s real mic → FrameRing |
| `AudioCaptureTest` | `captured_frames_can_be_opus_encoded` | PCM → Opus chain |
| `AudioCaptureTest` | `ring_basics` | FrameRing FIFO semantics |
| `AudioCaptureTest` | `ring_drops_oldest_when_full` | FrameRing overflow |
| `HandshakeTest` | `hello_contains_correct_fields_and_handshake_completes` | TCP Hello/HelloAck/Ready |
| `HandshakeTest` | `handshake_fails_gracefully_on_server_error_msg` | ErrorMsg handling |
| `LoopbackEndToEndTest` | `client_streams_real_audio_to_localhost_listener` | Full pipeline loopback |
| `PacketLayoutTest` | `header_magic_is_0xA1` | UDP magic byte |
| `PacketLayoutTest` | `header_version_is_0x10` | UDP version byte |
| `PacketLayoutTest` | `header_flags_is_0_without_marker` | Flags without marker |
| `PacketLayoutTest` | `header_flags_has_bit0_set_with_marker` | Marker bit |
| `PacketLayoutTest` | `header_payload_type_is_0x01` | Payload type field |
| `PacketLayoutTest` | `header_ssrc16_matches_configured_value` | SSRC16 field |
| `PacketLayoutTest` | `seq_increments_by_one_between_packets` | Sequence number +1 |
| `PacketLayoutTest` | `timestamp_advances_by_240_after_advanceTimestamp` | Timestamp +240 |
| `PacketLayoutTest` | `payload_follows_header_verbatim` | Payload bytes |
| `PacketLayoutTest` | `seq_wraps_from_0xFFFF_to_0x0000` | Seq wrap-around |

---

## 7. Known Issues and Mitigations

### 7.1 MIUI / Xiaomi Multicast Filtering

**Symptom:** Tapping "Discover" shows no servers for 30+ seconds even when the Mac server is running.

**Root cause:** Some MIUI builds drop mDNS multicast packets at the Wi-Fi driver layer (kernel network stack restriction for battery/privacy).

**Mitigations:**
- The app acquires a `WifiManager.MulticastLock("anymic")` on Discovery start, which fixes this on most MIUI versions.
- If still not working: enable "Developer Options → Wireless debugging" which tends to re-enable multicast routing.
- Fallback: use manual IP entry (not yet exposed in the T11 demo UI — planned for T12).

### 7.2 Wi-Fi AP Isolation / Client Isolation

**Symptom:** Discovery finds no servers even on the same SSID.

**Root cause:** Many enterprise and public Wi-Fi networks (also some home routers) enable AP Client Isolation, which prevents device-to-device LAN traffic including mDNS and UDP.

**Mitigation:** Disable AP isolation in router settings, or use a dedicated hotspot (phone hotspot connecting the Mac).

### 7.3 Android 12+ NsdManager Race Condition

**Symptom:** `onResolveFailed` with `errorCode=3` (NSD_FAILURE_ALREADY_ACTIVE).

**Root cause:** Android 12 NsdManager allows only one active resolve at a time. If two services are found simultaneously, the second resolve call fails.

**Mitigation:** The current `Discovery` implementation calls `resolveService` immediately on `onServiceFound`. A proper fix is to queue resolve calls (planned for T12). For manual testing, servers discovered after a short delay resolve correctly.

### 7.4 Android 13+ Bluetooth Permissions Interaction

**Symptom:** App crashes on launch with `BLUETOOTH_CONNECT` SecurityException on some Android 13 builds.

**Root cause:** Some Compose BOM components indirectly check Bluetooth state.

**Mitigation:** This does not affect audio streaming. The T11 demo UI uses basic Material3 components that avoid this path.

### 7.5 Firewall on macOS

**Symptom:** Android connects via TCP (handshake succeeds) but no audio reaches the Mac.

**Root cause:** macOS Application Firewall may block incoming UDP port 50127.

**Mitigation:**
```bash
# Check firewall
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --listapps | grep -i anymic

# Allow the server binary through the firewall:
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add /path/to/anymic-server
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp /path/to/anymic-server
```

Alternatively, disable the Application Firewall during testing: System Settings → Network → Firewall → Off.

---

## 8. Latency Measurement (Optional)

To measure end-to-end latency with the chirp test signal:

```bash
# On Mac: play a chirp from the Mac speaker while recording from the Android mic
ffmpeg -i tests/fixtures/chirp.wav -f avfoundation -i ":default" \
    -filter_complex "[0:a]adelay=500|500[delayed];[1:a][delayed]amix=2" \
    /tmp/latency_capture.wav

# Use the xcorr tool to compute latency
python3 tests/tools/xcorr_latency.py /tmp/latency_capture.wav
```

Expected result on a clean Wi-Fi network: 5–25 ms end-to-end (encoding 5ms + network 1–10ms + jitter buffer 1–5ms).

---

## 9. Cleanup

```bash
# Uninstall test app
adb uninstall com.anymic.app

# Stop server (Ctrl+C in the server terminal)
```
