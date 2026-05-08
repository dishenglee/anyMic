# Changelog

All notable changes to anyMic are documented in this file.  
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).  
Version scheme: [Semantic Versioning](https://semver.org/).

---

## [0.1.0] - 2026-05-08

First complete release milestone (T01–T15). macOS server + Android client 端到端可用，
主观听感通过，性能稳态达到 0 PLC / 0 stall / 0 sink underrun。

### Added

#### macOS Server
- `anymic-app`（原 `anymic-tauri`）：Rust + Tauri 2 桌面应用，启动时自动绑定 UDP/TCP/mDNS
- `anymic-core`：平台无关核心 crate，包含 packet codec、自适应 jitter buffer、Opus decoder、mDNS responder、`AudioSink` trait
- `anymic-audio-mac`：CoreAudio HAL 后端，直写 BlackHole 2ch 虚拟音频设备
- tokio multi-thread 主循环，UDP RX / Core / Audio 三线程分离
- 深色模式 Tauri UI，状态栏 UDP/TCP 连接指示

#### Windows Server (alpha)
- Windows server (alpha) — WASAPI sink writing to VB-CABLE Virtual Audio Cable, zero-cost (no kernel driver, no signing certificate). Cross-platform code in `anymic-audio-win`. Cargo check passes on macOS host targeting `x86_64-pc-windows-msvc`; full .exe build requires Windows machine or GitHub Actions Windows runner.

#### Android Client
- Kotlin + Jetpack Compose 完整 UI，支持 HomeScreen / DeviceListScreen / StatsScreen 导航
- `MicForegroundService`：后台麦克风采集，锁屏后持续推流
- `StreamingClient` Application 单例，生命周期与 Service 绑定
- libopus NDK 编译 + JNI 绑定（`OpusNative.kt`），Opus AUDIO 96kbps fullband
- `AudioRecord` 48kHz mono 采集 → Opus encode → UDP TX（5ms 帧间隔）
- mDNS 自动发现（Android NSD API，`_anymic._udp.local.`）

#### 协议与网络
- 自定义 12 字节 UDP 包头（anyMic v1）：magic 2B + flags 1B + ssrc16 2B + seq 2B + ts 4B + payload_len 1B
- protobuf TCP 控制通道：`Hello` / `HelloAck` / `Ready` / `Stats` / `Pong` 消息类型
- mDNS TXT 记录：`v=1, ctl=50128`，SRV 指向 UDP 50127
- `docs/protocol-v1.md`（815 行）完整协议规范

#### 测试基础设施
- 35+ Rust 单元测试 + `proptest` property-based 测试（jitter buffer、packet codec、PLC）
- 17+ Android instrumented 测试（`RemoteConnectTest`、`OpusNativeTest` 等）
- Python 端到端测试 orchestrator（`tests/`）：chirp 延迟测量、丢包率统计、PESQ 语音质量评分、频谱分析
- `Makefile` 统一入口：`test-server` / `test-android` / `test-e2e` / `test-quality`

### Performance（调优 Demo v1 → Demo A2）

经过 8 次迭代调优（详见 `docs/architecture.md` § 调优历程），最终稳态性能：

| 指标 | 值 |
|------|----|
| PLC 事件/s | **0** |
| Sink underrun/s | **0** |
| Stall/s | **0** |
| jitter p95 | 10 ms |
| Jitter Buffer 深度 | 100 ms（20 帧）|
| 总端到端延迟 | ~250 ms |

### Known Issues

- **Windows server (alpha)**：代码已写，需要 Windows 机器 build & 实测，macOS 开发机仅 cargo check 通过
- **Linux server 未实现**：规划在 M5 里程碑（PipeWire 路线）
- **iOS client 未实现**：规划在 M5（SwiftUI + AVAudioEngine）
- **USB / 蓝牙传输未实现**：M5 计划
- **总延迟 ~250ms**：高于 WO Mic（80–150ms）；M5 目标 < 80ms，路径为 Android 严格 5ms 节拍发包 + CoreAudio callback-driven server clock
- **控制通道握手 stub**：server 端 TCP Hello 响应为 MVP（"OK\n"），client fallback 到 default ssrc；M5 完整握手
- **Tauri UI**：当前使用 vanilla JS，M5 可升级至 Svelte/React

---

## [Unreleased]

规划中（M5 里程碑）：

- Windows server 实测验证（在 Windows 机器或 GitHub Actions Windows runner 上 build + e2e 测试）
- Linux server（PipeWire 节点）
- iOS client（SwiftUI + AVAudioEngine）
- 端到端延迟优化目标 < 80ms
- 完整 protobuf 握手（取代 MVP stub）
- USB 传输支持
