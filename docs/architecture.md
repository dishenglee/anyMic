# anyMic Architecture

**Version:** 1.0.0  
**Date:** 2026-05-08  

---

## 1. System Overview

anyMic streams microphone audio from an Android device to a macOS desktop, where it
appears as a virtual audio device that any application can use as its default
microphone input.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Android Device                                                               │
│                                                                             │
│  ┌─────────────┐    PCM 48kHz   ┌──────────────┐   Opus frames   ┌───────┐ │
│  │ AudioRecord │───────────────▶│ Opus Encoder │────────────────▶│  UDP  │ │
│  │  (HAL mic)  │                │  (libopus)   │                 │  TX   │ │
│  └─────────────┘                └──────────────┘                 └───┬───┘ │
│                                                                       │     │
│  ┌──────────────────────────────────────────────────────────────┐    │     │
│  │ anyMic Android Client                                        │    │     │
│  │  ┌─────────────┐  ┌───────────────┐  ┌────────────────────┐ │    │     │
│  │  │ mDNS Browse │  │ TCP Control   │  │ UDP Data Sender    │ │    │     │
│  │  │ (NSD API)   │  │ (Protobuf)    │  │ (12B hdr + Opus)   │ │    │     │
│  │  └─────────────┘  └───────────────┘  └────────────────────┘ │    │     │
│  └──────────────────────────────────────────────────────────────┘    │     │
└───────────────────────────────────────────────────────────────────────┼─────┘
                                                                        │
                              Wi-Fi (LAN) ────────────────────────────── │
                                                    UDP :50127          │
                                                    TCP :50128          │
                                                                        ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ macOS Desktop                                                               │
│                                                                             │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │ anyMic macOS Server (Tauri + Rust)                                   │  │
│  │                                                                      │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌────────────────────────────┐  │  │
│  │  │ mDNS Advert │  │ TCP Listener │  │ UDP Listener               │  │  │
│  │  │ (Bonjour)   │  │ :50128       │  │ :50127                     │  │  │
│  │  └─────────────┘  └──────┬───────┘  └─────────────┬──────────────┘  │  │
│  │                          │ session                 │ ssrc16 demux   │  │
│  │                          ▼                         ▼                │  │
│  │  ┌──────────────────────────────────────────────────────────────┐  │  │
│  │  │ anymic-core (platform-agnostic)                              │  │  │
│  │  │  ┌───────────────┐  ┌─────────────┐  ┌──────────────────┐   │  │  │
│  │  │  │ Session Mgr   │  │Jitter Buffer│  │ Opus Decoder     │   │  │  │
│  │  │  │ (SSRC, state) │  │ (adaptive)  │  │ (libopus PLC)    │   │  │  │
│  │  │  └───────────────┘  └─────────────┘  └────────┬─────────┘   │  │  │
│  │  └───────────────────────────────────────────────────────┼───────┘  │  │
│  │                                                           │ PCM      │  │
│  │  ┌──────────────────────────────────────────────────────────────┐  │  │
│  │  │ anymic-audio-mac (CoreAudio / AudioUnit implementation)      │  │  │
│  │  │  ┌─────────────────────────────────────────────────────────┐ │  │  │
│  │  │  │ BlackHole virtual audio driver (or similar)             │ │  │  │
│  │  │  └───────────────────────────────┬─────────────────────────┘ │  │  │
│  │  └────────────────────────────────────────────────────────────── │  │  │
│  └───────────────────────────────────────────────────────────────────┼──┘  │
│                                                                       │     │
│  ┌────────────────────────────────────────────────────────────────────┐    │
│  │ macOS Audio System                                                 │    │
│  │                                                                    │    │
│  │  BlackHole ──────────────▶ System Microphone Input (default)      │    │
│  │                                    │                              │    │
│  │                           ┌────────┴───────────┐                  │    │
│  │                           │  Any Application   │                  │    │
│  │                           │  (Zoom, Teams,     │                  │    │
│  │                           │   Discord, DAW...) │                  │    │
│  │                           └────────────────────┘                  │    │
│  └────────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Server Internal Module Dependency Graph

The server is structured as two layers: a platform-agnostic core and OS-specific
audio backend implementations.

```
┌─────────────────────────────────────────────────────────────────────────┐
│ anymic-server (Tauri binary)                                            │
│  • System tray UI                                                       │
│  • Settings persistence                                                 │
│  • OS lifecycle management                                              │
└─────────────────────┬───────────────────────────────────────────────────┘
                      │ depends on
                      ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ anymic-core  (platform-agnostic Rust crate)                             │
│                                                                         │
│  ┌──────────────┐   ┌──────────────┐   ┌───────────────────────────┐  │
│  │  UdpReceiver │   │  TcpControl  │   │    SessionManager         │  │
│  │              │   │              │   │                           │  │
│  │  • Binds     │   │  • Accepts   │   │  • SSRC table             │  │
│  │    :50127    │   │    :50128    │   │  • State machine          │  │
│  │  • Validates │   │  • Framing   │   │    per client             │  │
│  │    magic/    │   │    (u32 len  │   │  • Reconnect window       │  │
│  │    ssrc16    │   │    prefix)   │   │    timer                  │  │
│  │  • Demux     │   │  • Protobuf  │   │  • Max clients guard      │  │
│  └──────┬───────┘   │    decode    │   └────────────┬──────────────┘  │
│         │           └──────┬───────┘                │                 │
│         │                  │ session events          │                 │
│         │                  └────────────────────────┘                 │
│         │ raw Opus frames                                              │
│         ▼                                                              │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────────────────┐  │
│  │ JitterBuffer │──▶│ OpusDecoder  │──▶│  AudioOutput trait       │  │
│  │              │   │              │   │  (platform abstraction)   │  │
│  │  • Adaptive  │   │  • libopus   │   │                          │  │
│  │    target    │   │  • PLC on    │   │  fn write_pcm(           │  │
│  │    depth     │   │    frame gap │   │    &mut self,            │  │
│  │  • EMA       │   │  • 48kHz     │   │    samples: &[i16])      │  │
│  │    smoothing │   │    mono out  │   │                          │  │
│  └──────────────┘   └──────────────┘   └──────────────────────────┘  │
│                                                       ▲               │
│                                     implements trait  │               │
└───────────────────────────────────────────────────────┼───────────────┘
                                                        │
          ┌─────────────────────────────────────────────┤
          │                                             │
          ▼                                             ▼
┌────────────────────────┐               ┌─────────────────────────────┐
│ anymic-audio-mac       │               │ anymic-audio-win (future)   │
│                        │               │                             │
│ CoreAudio / AudioUnit  │               │ WASAPI / PortAudio          │
│ BlackHole IAC driver   │               │ Virtual Cable driver        │
└────────────────────────┘               └─────────────────────────────┘
```

**Dependency rules:**
- `anymic-core` has zero OS-specific dependencies. It compiles on macOS, Linux, and Windows.
- `anymic-audio-<os>` depends on `anymic-core` (for the `AudioOutput` trait) and OS audio APIs.
- `anymic-server` depends on both. At compile time, only the target platform's audio backend is compiled in.
- `anymic-core` does NOT depend on Tauri or any UI framework.

---

## 3. Key Sequence: Startup → Discovery → Handshake → Streaming → Teardown

```
macOS Server                      Android Client               User
     │                                  │                        │
     │  [App launch]                    │                        │
     │                                  │                        │
     │──bind UDP :50127─────────────────│                        │
     │──bind TCP :50128─────────────────│                        │
     │──mDNS register───────────────────│  ← _anymic._udp.local. │
     │                                  │                        │
     │                                  │       [App launch]     │
     │                                  │◀──────────────────────│
     │                                  │                        │
     │                                  │──mDNS browse──────────▶│
     │◀─────────────────────────────────│  _anymic._udp.local.  │
     │  (mDNS response)                 │                        │
     │─────────────────────────────────▶│                        │
     │  SRV: port 50127                 │                        │
     │  TXT: ctl=50128, v=1, ...        │                        │
     │                                  │                        │
     │                                  │  [User taps server]   │
     │                                  │◀──────────────────────│
     │                                  │                        │
     │  ┌── HANDSHAKE ──────────────────┼───────────────────┐   │
     │  │                               │                   │   │
     │◀─│────────── TCP connect :50128──│                   │   │
     │◀─│────────── Hello{...}──────────│                   │   │
     │──│────────── HelloAck{ssrc,...}──▶                   │   │
     │◀─│────────── Ready{}─────────────│                   │   │
     │  └───────────────────────────────┘                   │   │
     │                                                       │   │
     │  ┌── STREAMING ──────────────────────────────────┐   │   │
     │  │                                               │   │   │
     │◀─│── UDP [A1 10 01 01 ...] ─────────────────────│   │   │
     │◀─│── UDP [A1 10 00 01 ...] ─────────────────────│   │   │
     │  │   (200 packets/s, 5ms frames)                │   │   │
     │◀─│── TCP Stats{rtt=4,lost=0,...} ───────────────│   │   │
     │──│── TCP Pong{server_ts,...}────────────────────▶   │   │
     │  │   (every 1s)                                 │   │   │
     │  └───────────────────────────────────────────── │   │   │
     │                                                       │   │
     │                                  │  [User stops app] │   │
     │                                  │◀──────────────────│   │
     │                                  │                        │
     │◀─────────────────────────────────│── TCP Disconnect{}     │
     │──────────────────────────────────▶ (TCP FIN)              │
     │                                  │                        │
     │──mDNS keep advertising───────────│                        │
     │  (ready for next client)         │                        │
```

---

## 4. Socket Inventory and Lifecycle

### 4.1 Socket Summary

| Socket | Type | Port | Owner | Lifetime |
|--------|------|------|-------|---------|
| Data listener | UDP | 50127 | Server | Application lifetime |
| Control listener | TCP (accept loop) | 50128 | Server | Application lifetime |
| Per-session control | TCP (connected) | ephemeral (client) | Session | Hello → Disconnect |
| Per-session data | UDP (logical, not separate socket) | 50127 | UdpReceiver | Session (ssrc16 active) |

### 4.2 Server Socket Lifecycle

```
Application start
      │
      ├──▶ UdpSocket::bind("0.0.0.0:50127")   ← one socket, lives forever
      │
      ├──▶ TcpListener::bind("0.0.0.0:50128") ← one listener, lives forever
      │
      │    loop: TcpListener::accept()
      │                │
      │                ▼
      │         TcpStream (per client)
      │                │
      │         read Hello
      │                │
      │         create Session { ssrc16, state, ... }
      │                │
      │         send HelloAck
      │                │
      │         read Ready
      │                │
      │         [STREAMING]
      │         UDP packets dispatched by ssrc16 → Session
      │         Stats/Pong on TcpStream
      │                │
      │         Disconnect or keepalive timeout
      │                │
      │         Session::drop() → remove ssrc16 from table
      │         TcpStream::close()
      │
Application quit
      │
      ├──▶ mDNS goodbye (TTL=0)
      ├──▶ TcpListener::close()
      └──▶ UdpSocket::close()
```

### 4.3 Client Socket Lifecycle

```
App start
  │
  ├──▶ mDNS browse (NSD on Android)
  │
  │    On server discovered / user selects:
  │
  ├──▶ TcpStream::connect(server_ip, ctl_port)
  ├──▶ send Hello
  ├──▶ receive HelloAck
  ├──▶ UdpSocket::bind("0.0.0.0:0")  ← ephemeral port
  ├──▶ UdpSocket::connect(server_ip, 50127)
  ├──▶ send Ready (TCP)
  │
  │    [STREAMING]
  │    AudioRecord → Opus encode → UDP send loop (5ms interval)
  │    Stats timer → TCP send every 1000ms
  │    Pong receive → update RTT
  │
  │    On disconnect / error:
  │
  ├──▶ UdpSocket::close()
  ├──▶ TcpStream::close()
  │
  │    [RECONNECT if within window]
  └──▶ repeat from TcpStream::connect
```

---

## 5. Data Flow Within the Server (Per Audio Frame)

```
Network RX thread          Core thread              Audio thread
       │                        │                        │
UDP recv()                      │                        │
 12-byte header parse           │                        │
 ssrc16 lookup → Session        │                        │
       │                        │                        │
       │── enqueue(OpusFrame) ──▶                        │
       │                        │                        │
       │               JitterBuffer::push(frame)         │
       │               (schedule by timestamp)           │
       │                        │                        │
       │               JitterBuffer::pop()               │
       │               (at playout time)                 │
       │                        │                        │
       │               OpusDecoder::decode(frame)        │
       │               → [i16; 240]  PCM samples         │
       │                        │── write_pcm(samples) ──▶
       │                        │                        │
       │                        │               AudioUnit callback
       │                        │               → CoreAudio ring buffer
       │                        │               → BlackHole virtual device
       │                        │               → System audio graph
```

Frame arrival to audio output: target latency ≤ 15 ms (jitter buffer 1–3 frames).

---

## 6. mDNS and Network Layer

```
┌─────────────────────────────────────────────────────────────┐
│ Network Layer                                               │
│                                                             │
│  Multicast (224.0.0.251 / ff02::fb)   Unicast              │
│  ┌──────────────────────────────┐     ┌────────────────┐   │
│  │  mDNS :5353                  │     │  UDP :50127    │   │
│  │  _anymic._udp.local.         │     │  (audio data)  │   │
│  │  TXT: v=1, ctl=50128, ...    │     └────────────────┘   │
│  └──────────────────────────────┘                          │
│                                        ┌────────────────┐   │
│                                        │  TCP :50128    │   │
│                                        │  (control)     │   │
│                                        └────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

All three ports operate on the same Wi-Fi interface. No routing, NAT, or firewall
traversal is required; anyMic is a strictly LAN-local protocol in v1.

---

## 7. 调优历程（Performance Tuning Journey）

本节记录从初版原型到 Demo A2 发布版本的 8 次性能迭代过程。每次迭代均有明确的症状、根因诊断、代码修改位置和实测数据。

---

### 7.1 初版（Demo v1）：VOIP 32kbps + Stall 补零

**症状（主观）**：用户说话时持续出现金属质感杂音，低频完全缺失，听感接近电话。

**数据诊断**：
- PLC/s = 8–15（jitter MIN=1 帧，几乎任何网络抖动都触发 stall）
- Stall 处理：写入 `[0i16; 240]` 静音样本，边界处 PCM 幅值跳变造成咔哒

**代码修改**：
- `android/app/src/androidTest/java/.../RemoteConnectTest.kt`：`encoder.setBitrate(32000)` → 仅为初版基准
- `server/anymic-core/src/server.rs` 的 stall 分支：`sink.write_pcm(&[0i16; 240])`

**Trade-off**：32kbps VOIP 模式针对语音优化，带宽窄；但频响截断 < 8kHz，fullband 质感完全丢失。

---

### 7.2 Demo v2：Opus AUDIO 96kbps Fullband

**症状（主观）**：解决频谱缺失问题。用户反馈"明显变清晰，但仍有断续咔哒"。

**数据诊断**：
- PLC/s 仍为 8–12（jitter buffer MIN=1 未改）
- 频谱分析：0–20kHz 全频段有效，PESQ 分数从 2.1 提升到 3.4

**代码修改**：
- `android/app/src/androidTest/java/.../RemoteConnectTest.kt`：`encoder.setBitrate(96000)`
- `android/app/src/main/java/.../OpusNative.kt`：`APPLICATION_AUDIO`（替换 `APPLICATION_VOIP`）

**Trade-off**：带宽从 32kbps 增至 96kbps，5GHz Wi-Fi 完全可承受；AUDIO 模式禁用语音专用 DTX，持续发包更规律。

---

### 7.3 Demo v3：Jitter Buffer MIN=4（20ms）+ Stall 走 PLC

**症状（主观）**：仍有断续，但咔哒声频率减少约 40%。

**数据诊断**：
- PLC/s ≈ 3–5（MIN=4 提供 20ms 缓冲，吸收部分抖动）
- Stall 改用 Opus PLC（`decoder.decode_fec()`），边界较补零平滑

**代码修改**：
- `server/anymic-core/src/jitter.rs`：`const MIN_TARGET: u32 = 4;`
- `server/anymic-core/src/server.rs` stall 分支：替换为 `decoder.decode(None)` 触发 PLC

**Trade-off**：PLC 在丢包率 < 5% 时听感尚可；jitter buffer 深度 20ms 仍不足以应对 Android 的突发 burst（实测 burst 长度可达 60ms+）。

---

### 7.4 Demo v4：Jitter Buffer MIN=10（50ms）

**症状（主观）**："卡顿减少了，但偶尔还是有一下"。

**数据诊断**：
- PLC/s ≈ 1–2（50ms 缓冲覆盖大多数 burst，极端 burst 仍穿透）
- 总延迟增加：从 ~160ms 到 ~200ms

**代码修改**：
- `server/anymic-core/src/jitter.rs`：`const MIN_TARGET: u32 = 10;`

**Trade-off**：延迟代价换稳定性；MIN=10 在实验室 Wi-Fi 下覆盖率约 95%，但 p99 burst 仍可穿透。

---

### 7.5 Demo v5：PLC ↔ Frame 2ms Cross-fade（96 样本）

**症状（主观）**：咔哒声频率再减约 60%；偶尔仍有轻微不连续感。

**根因分析**：PLC 帧与下一正常帧之间，Opus 内部状态重同步导致约 2–5ms 的相位跳变，PCM 边界幅值不连续。

**代码修改**：
- `server/anymic-core/src/server.rs`，PLC/正常帧切换处增加：

```rust
// cross_fade_samples = 96  (2ms at 48kHz)
for i in 0..CROSS_FADE_SAMPLES {
    let alpha = i as f32 / CROSS_FADE_SAMPLES as f32;
    output[i] = (prev_frame[i] as f32 * (1.0 - alpha)
               + new_frame[i] as f32 * alpha) as i16;
}
```

**Trade-off**：cross-fade 引入 2ms 额外处理，但完全在帧内完成，不增加端到端延迟。

---

### 7.6 Demo v6：Sink Prebuffer 50ms → 100ms + Underrun Hold

**症状（主观）**："基本可用，但有时候开头一段会发出短暂噪音"。

**数据诊断**：
- sink_underrun/s ≈ 2–4（prebuffer 50ms 在启动预热期不足）
- Underrun 原有处理：写 0 样本，产生静音咔哒

**代码修改**：
- `server/anymic-audio-mac/src/blackhole.rs`：`PREBUFFER_MS = 100`
- Underrun 处理改为 hold last L/R 样本（重复末帧而非补零）

**Trade-off**：prebuffer 100ms 增加启动延迟约 50ms，但消除预热期 underrun。Hold 策略在短暂 underrun（< 5ms）时几乎无感知。

---

### 7.7 Demo A：Jitter Buffer MIN=20（100ms）+ Sink Prebuffer 200ms

**症状（主观）**："听起来很稳定！就是偶尔 BlackHole 侧会有轻微 underrun"。

**数据诊断**：
- PLC/s = **0**（100ms jitter buffer 完全覆盖 Android burst）
- sink_underrun/s ≈ 1（时钟漂移：CoreAudio 回调速率与 server 推帧速率存在 ±0.1% 差异）
- 总延迟：~240ms

**代码修改**：
- `server/anymic-core/src/jitter.rs`：`const MIN_TARGET: u32 = 20; const MAX_TARGET: u32 = 32;`
- `server/anymic-audio-mac/src/blackhole.rs`：`PREBUFFER_MS = 200`

**Trade-off**：延迟 240ms 已接近 WO Mic 150ms 的 2 倍；时钟漂移 underrun 成为新瓶颈。

---

### 7.8 Demo A2：Sink Top-up 机制（最终版）

**症状（主观）**："完全没有可感知的问题了！" 30 秒连续说话，QuickTime 录制回放清晰可懂。

**根因分析**：CoreAudio 音频回调以固定速率拉取样本，而 server 以网络包到达速率推帧，两者存在 ±0.1% 时钟差。长时间运行后 sink ring buffer 水位持续下降，最终触发 underrun。

**修复方案**：在每次 `write_pcm` 后检查 ring buffer 水位，若低于 30ms 阈值，重复最近一帧（最多 10 帧，50ms）直到水位恢复。

**代码修改**：
- `server/anymic-audio-mac/src/blackhole.rs`，`write_pcm` 尾部增加：

```rust
// Top-up: 水位 < 30ms 时重复刚推帧，最多 10 次
const TOP_UP_THRESHOLD_FRAMES: usize = 6;   // 30ms = 6 帧
const TOP_UP_MAX_REPEAT: usize = 10;
let mut repeat = 0;
while self.ring_available_frames() < TOP_UP_THRESHOLD_FRAMES
    && repeat < TOP_UP_MAX_REPEAT
{
    self.ring_push(&last_frame);
    repeat += 1;
}
```

**实测数据（Demo A2 稳态，每 5s 采样）**：

```
udp_pkts/5s   = 1000  (200 pkt/s，正常 5ms 节拍)
frames/5s     = 1000  (server tick 完美匹配 UDP 速率)
PLC 稳态      = 0/s   (jitter buffer 100ms 完全吸收 Android burst)
Stall 稳态    = 0/s   (网络无突发故障)
sink_underrun = 0/s   (top-up 维持水位)
jitter_p95    = 10 ms (Wi-Fi 5GHz 抖动稳定)
target_depth  = 20 帧 (100ms 自适应深度)
```

**Trade-off**：top-up 最多引入 50ms 额外"假帧"，在时钟漂移速率 < 0.1% 时，实际每分钟 top-up < 3 帧，听感不可感知。

---

### 7.9 最终性能数据 + 设计决策表

| 参数 | 值 | 文件 | 决策依据 |
|------|----|------|---------|
| Opus bitrate | 96 kbps | `RemoteConnectTest.kt` | VOIP 32k 带宽窄，频响截断 < 8kHz |
| Opus application | `APPLICATION_AUDIO` | `OpusNative.kt` | fullband 频响，禁用 DTX，发包更规律 |
| Jitter `MIN_TARGET` | 20 帧（100ms） | `jitter.rs` | Android burst 长度实测最大 80ms |
| Jitter `MAX_TARGET` | 32 帧（160ms） | `jitter.rs` | 极端网络留 60ms 头空间 |
| PLC ↔ Frame cross-fade | 2ms（96 samples） | `server.rs` | 边界相位连续，消除咔哒 |
| Stall 处理策略 | Opus PLC（`decode(None)`） | `server.rs` | 替零样本产生宽带噪声 |
| Sink ring buffer 容量 | 16384 stereo samples（~170ms）| `blackhole.rs` | 启动预热期容忍大 burst |
| Sink prebuffer | 200ms | `blackhole.rs` | warmup 阶段不触发 underrun |
| Sink underrun 填充 | hold last L/R 样本 | `blackhole.rs` | 短暂 underrun 无咔哒 |
| Sink top-up 阈值 | 30ms（6 帧）| `blackhole.rs` | 时钟漂移补偿触发点 |
| Sink top-up 最大重复 | 10 帧（50ms）| `blackhole.rs` | 避免 runaway 正反馈 |

### 7.10 已知限制与后续优化方向（M5）

| 限制 | 当前值 | M5 目标 | 优化路径 |
|------|--------|---------|---------|
| 总端到端延迟 | ~250ms | < 80ms | Android 严格 5ms 节拍 + CoreAudio callback-driven server clock |
| Windows/Linux server | 未实现 | M5 | VB-CABLE（Windows）/ PipeWire 节点（Linux）|
| iOS client | 未实现 | M5 | SwiftUI + AVAudioEngine |
| TCP 握手 | MVP stub（"OK\n"）| M5 | 完整 protobuf 握手状态机 |
| Tauri UI | vanilla JS | M5（可选）| Svelte / React 组件化 |
