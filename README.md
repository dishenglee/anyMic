# anyMic

(Logo TBD)

**anyMic** 是跨平台开源无线麦克风工具，把你的智能手机变成电脑的外置麦克风。
项目对标 [WO Mic](https://wolicheng.com/womic/)，并补齐其在 macOS 上的缺失支持，同时提供更优秀的编解码质量（Opus fullband 96kbps）与零配置局域网发现（mDNS）。

核心设计目标：**开源、低延迟、免驱安装**。macOS server 通过 BlackHole 虚拟音频设备暴露给系统，Zoom、微信、QuickTime 等任意应用可直接选用，无需额外配置。

协议与实现完全公开，Windows/Linux/iOS 支持列入 M5 里程碑。

---

## 功能特性

- **Opus fullband 编解码**：96kbps AUDIO 模式，48kHz mono，频响 0–20kHz，远优于竞品 G.711/G.722
- **自适应 Jitter Buffer**：100ms 自适应深度，EMA jitter 估计，完全吸收 Android burst 抖动
- **零配置 mDNS 发现**：`_anymic._udp.local.` 服务注册，手机端一键 Discover，无需手动填 IP
- **BlackHole 虚拟设备集成**：server 直写 BlackHole 2ch，任何 macOS 应用可作为麦克风输入
- **Foreground Service 后台采集**：Android 侧 MicForegroundService，锁屏后持续推流
- **protobuf TCP 控制通道**：Hello/HelloAck/Ready/Stats/Pong 握手协议，支持 RTT 统计
- **PLC 丢包隐藏**：Jitter stall 时调用 Opus PLC 而非补零，消除卡顿噪音
- **2ms 渐变平滑**：PLC 帧与正常帧边界 96 样本 cross-fade，消除相位跳变咔哒声
- **Sink Top-up 补偿**：BlackHole ring 水位低于 30ms 时自动重复末帧，消除时钟漂移 underrun
- **35+ Rust 单测 + proptest**，17+ Android instrumented 测试，Python 声学端到端测试（PESQ + 频谱）
- **MIT 开源**

---

## 平台支持矩阵

|            | macOS          | Windows           | Linux          | Android        | iOS            |
|------------|----------------|-------------------|----------------|----------------|----------------|
| **Server** | ✅ v0.1        | ✅ v0.1 alpha     | M5 计划        | —              | —              |
| **Client** | —              | —                 | —              | ✅ v0.1        | M5 计划        |

> macOS server 需要 Apple Silicon（M 系芯片），macOS 12 Monterey 及以上。  
> Android client 需要 Android 8.0（API 26）及以上。  
> Windows alpha 含义：代码已写，需要 Windows 机器 build & 实测，macOS 开发机仅 `cargo check --target x86_64-pc-windows-msvc` 通过。

---

## 快速开始

> 详细装机指引见 [INSTALL.md](INSTALL.md)。

### 前置依赖（一次性）

```bash
# Mac 端
brew install blackhole-2ch ffmpeg
curl https://sh.rustup.rs -sSf | sh          # Rust 1.75+
brew install node                             # Node 20+
# Android SDK 和 NDK 通过 Android Studio 安装，或手动设置 ANDROID_HOME
```

### 五步启动

```bash
# 步骤 1：验证 server 可以编译并通过单测
make test-server

# 步骤 2：启动 macOS Tauri 应用（server 随 UI 启动）
cd server && cargo run -p anymic-app --release

# 步骤 3：构建并安装 Android APK（USB 连接手机）
make test-android   # 等价于 cd android && ./gradlew installDebug

# 步骤 4：手机打开 anyMic，点击 Discover，选择出现的 Mac 服务器

# 步骤 5：在 Mac 任意应用（Zoom / 微信 / QuickTime）中
#         将输入设备设置为 "BlackHole 2ch"
```

---

## 架构概览

```
手机（Android）                          Mac（macOS）
┌─────────────────────────┐             ┌───────────────────────────────────────┐
│  AudioRecord (48kHz)    │             │  anyMic macOS Server (Rust + Tauri 2) │
│         │               │             │  ┌───────────┐  ┌──────────────────┐  │
│  libopus Encoder        │  Wi-Fi LAN  │  │ UDP :50127│  │ Adaptive Jitter  │  │
│  AUDIO 96kbps           │────────────▶│  │ TCP :50128│  │ Buffer (100ms)   │  │
│         │               │  Opus UDP   │  └───────────┘  └────────┬─────────┘  │
│  UDP TX (5ms 帧)        │             │  mDNS _anymic._udp.local.│            │
│  TCP 控制通道           │             │                  Opus Decoder (PLC)   │
│  mDNS Discover          │             │                           │            │
└─────────────────────────┘             │                  BlackHole 2ch Ring   │
                                        └───────────────────────────┬───────────┘
                                                                    │
                                                         ┌──────────▼──────────┐
                                                         │  macOS 音频系统      │
                                                         │  Zoom / 微信 / DAW  │
                                                         └─────────────────────┘
```

详细架构设计见 [docs/architecture.md](docs/architecture.md)。

---

## 实测性能数据

以下数据来自 Demo A2 版本稳态运行（Wi-Fi 5GHz，MacBook + Pixel 手机，`/tmp/anymic_demoA2_*` 日志）：

| 指标 | 值 | 说明 |
|------|----|------|
| UDP 包/5s | 1000 | 200 pkt/s，正常 5ms 节拍 |
| 解码帧/5s | 1000 | server tick 完美匹配 UDP 速率 |
| PLC 稳态 | **0 /s** | jitter buffer 100ms 完全吸收 Android burst |
| Stall 稳态 | **0 /s** | 网络无突发故障 |
| sink underrun | **0 /s** | top-up 机制维持 ring 水位 |
| jitter p95 | 10 ms | Wi-Fi 抖动稳定 |
| Jitter Buffer 深度 | 20 帧（100ms）| 自适应目标深度 |
| 总延迟 | ~250 ms | M5 优化目标 < 80ms |

主观听感：用户对手机说话 30 秒，QuickTime 录制 BlackHole 回放清晰可懂，产品可用。

---

## 竞品对比

| 功能 / 产品             | WO Mic       | AudioRelay      | DroidCam         | **anyMic**        |
|------------------------|--------------|-----------------|------------------|-------------------|
| macOS Server           | ❌            | ✅               | ✅ (视频为主)     | ✅ v0.1            |
| Windows Server         | ✅            | ✅               | ✅               | ✅ v0.1 alpha      |
| Linux Server           | ❌            | ❌               | ❌               | M5 计划            |
| Android Client         | ✅            | ✅               | ✅               | ✅ v0.1            |
| iOS Client             | ✅            | ✅               | ✅               | M5 计划            |
| 开源                   | ❌ 闭源       | ❌ 闭源          | ❌ 闭源          | ✅ MIT             |
| 价格                   | 免费 + 付费驱动 | 免费试用 + 订阅 | 免费 + 付费版     | **完全免费**       |
| 编解码                 | G.711 / PCM  | Opus（未公开码率）| H.264（视频）    | Opus 96kbps fullband |
| mDNS 零配置发现        | ❌ 需填 IP    | ✅               | ✅               | ✅                 |
| Wi-Fi 传输             | ✅            | ✅               | ✅               | ✅                 |
| USB 传输               | ✅            | ❌               | ✅               | M5 计划            |
| 协议公开               | ❌            | ❌               | ❌               | ✅ docs/protocol-v1.md |
| 自适应 Jitter Buffer   | ❌ 不透明     | ❌ 不透明        | ❌ 不透明        | ✅ 100ms 自适应    |
| PLC 丢包隐藏           | ❌            | ❌ 不透明        | —                | ✅ Opus PLC + cross-fade |
| 虚拟设备驱动           | 付费 ($7.99)  | 内置             | 内置             | BlackHole（MIT 免费）|

> WO Mic macOS 驱动需单独付费购买；anyMic 依赖 BlackHole 开源虚拟音频设备（免费）。

---

## 项目结构

```
anyMic/
├── server/                    # macOS server（Rust + Tauri 2）
│   ├── anymic-core/           # 平台无关核心：jitter buffer、Opus decoder、mDNS
│   ├── anymic-audio-mac/      # CoreAudio / BlackHole 音频后端
│   └── anymic-tauri/          # Tauri shell + tokio 主循环 + UI
├── android/                   # Android client（Kotlin + Compose）
│   ├── app/src/main/          # UI、Service、StreamingClient、mDNS
│   └── app/src/androidTest/   # Instrumented 测试
├── proto/                     # protobuf 控制协议定义
├── tests/                     # Python 端到端测试 orchestrator（PESQ + 频谱）
└── docs/
    ├── protocol-v1.md         # 协议规范（815 行）
    ├── architecture.md        # 系统架构与调优历程
    └── manual-e2e.md          # 手动端到端测试指引
```

---

## 开发参与

欢迎 PR 和 Issue！请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

```bash
# 运行所有测试
make test-server      # Rust 单测 + proptest（35+ 用例）
make test-android     # Android instrumented 测试（17+ 用例）
make test-e2e         # Python 声学链路端到端（chirp 延迟 + 包损）
make test-quality     # PESQ + 频谱分析
```

---

## 致谢

- [BlackHole](https://github.com/ExistentialAudio/BlackHole)（MIT）— 零成本 macOS 虚拟音频设备
- [libopus](https://opus-codec.org/)（BSD-3-Clause）— Android NDK JNI 编解码器
- [Tauri](https://tauri.app/)（Apache-2.0）— macOS 跨平台桌面框架
- [Anthropic Claude Code](https://anthropic.com/) — 全程 AI 辅助编写与调试

---

## License

MIT — 见 [LICENSE](LICENSE)。
