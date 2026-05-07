# anyMic

anyMic is a cross-platform open-source tool that turns your smartphone into a wireless microphone for your computer. It targets feature parity with [WO Mic](https://wolicheng.com/womic/) while adding first-class **macOS** support that WO Mic lacks.

## Feature Comparison

| Feature                  | WO Mic | anyMic |
|--------------------------|--------|--------|
| Windows Server           | ✅      | 🔜     |
| macOS Server             | ❌      | ✅ (Phase 1) |
| Linux Server             | ❌      | 🔜     |
| Android Client           | ✅      | ✅ (Phase 1) |
| iOS Client               | ✅      | 🔜     |
| Wi-Fi transport          | ✅      | ✅     |
| USB transport            | ✅      | 🔜     |
| Bluetooth transport      | ✅      | 🔜     |
| Opus codec               | ❌      | ✅     |
| mDNS auto-discovery      | ❌      | ✅     |
| Open source              | ❌      | ✅ MIT |

## Architecture

- **Desktop Server** — Rust + Tauri 2, receives audio over UDP and injects it into a virtual audio device (BlackHole on macOS).
- **Android Client** — Kotlin + Jetpack Compose, captures microphone audio, encodes with Opus, and streams over UDP.
- **Transport** — Opus-over-UDP with mDNS for zero-config server discovery on the local network.

## Installation (placeholder)

> Installers and detailed setup guides will be added in a later milestone. For now, build from source (see below).

### Build from source — Server (macOS)

```bash
# Requires Rust 1.94+, Node.js 20+
cd server
cargo tauri build
```

### Build from source — Android Client

```bash
cd android
./gradlew assembleRelease
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for PR and commit guidelines.

## License

MIT — see [LICENSE](LICENSE).
