# anyMic 安装指引

本文档提供从零开始构建并运行 anyMic 的完整步骤，包含依赖安装、构建、首次连接和故障排查。

---

## 1. 系统要求

### Mac（Server 端）

| 要求 | 最低版本 |
|------|---------|
| macOS | 12 Monterey（Apple Silicon 推荐）|
| 芯片 | Apple M1 / M2 / M3 / M4（Intel 未测试）|
| Rust | 1.75 及以上（见 `rust-toolchain.toml`）|
| Node.js | 20 及以上（Tauri CLI 依赖）|
| Java | 17 及以上（Android Gradle 依赖）|
| 网络 | Wi-Fi 5GHz，与手机同一子网 |

### Android（Client 端）

| 要求 | 最低版本 |
|------|---------|
| Android | 8.0 Oreo（API 26）及以上 |
| 权限 | `RECORD_AUDIO`、`FOREGROUND_SERVICE`、`INTERNET` |
| 网络 | 与 Mac 在同一 Wi-Fi 网段（支持多播）|

---

## 2. Mac 端依赖安装

### 2.1 Homebrew

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

### 2.2 BlackHole 虚拟音频设备

```bash
brew install blackhole-2ch
```

安装后在 **系统设置 → 声音 → 输出** 中可看到 "BlackHole 2ch"。

### 2.3 ffmpeg（用于端到端测试中生成/分析音频）

```bash
brew install ffmpeg
```

验证安装：

```bash
ffmpeg -list_devices true -f avfoundation -i "" 2>&1 | grep -i blackhole
# 应输出类似：[AVFoundation indev] [n] BlackHole 2ch
```

### 2.4 Rust 工具链

```bash
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
rustup show    # 验证版本，项目 rust-toolchain.toml 会自动 pin 版本
```

### 2.5 Node.js

```bash
brew install node
node -v    # 应为 v20 或更高
```

### 2.6 Java（Android Gradle 构建需要）

```bash
brew install openjdk@17
# 或通过 Android Studio 自带 JDK
```

### 2.7 Android SDK 和 NDK

推荐通过 **Android Studio** 安装，SDK 路径通常为 `~/Library/Android/sdk`。

```bash
# 设置环境变量（写入 ~/.zshrc 或 ~/.bashrc）
export ANDROID_HOME="$HOME/Library/Android/sdk"
export PATH="$ANDROID_HOME/tools/bin:$ANDROID_HOME/platform-tools:$PATH"
```

所需 NDK 版本：项目 `android/local.properties` 中指定，通常为 NDK 25+。  
在 Android Studio → SDK Manager → SDK Tools → NDK (Side by side) 中安装。

### 2.8 Python 虚拟环境（用于端到端测试）

```bash
python3 -m venv tests/venv
source tests/venv/bin/activate
pip install -r tests/requirements.txt   # 包含 pesq、scipy、numpy 等
```

---

## 3. Android 端准备

### 3.1 开启 USB 调试

1. 进入 **设置 → 关于手机**，连续点击「版本号」7 次，启用开发者模式
2. 进入 **设置 → 开发者选项**，打开「USB 调试」

验证 ADB 连接：

```bash
adb devices
# 应显示设备序列号和 "device" 状态
```

### 3.2 RECORD_AUDIO 权限

首次启动 app 时，Android 会弹出麦克风权限请求，点击「允许」。  
如需手动授权：

```bash
adb shell pm grant com.anymic.app android.permission.RECORD_AUDIO
```

### 3.3 MIUI / 小米设备多播例外（如适用）

MIUI 默认拦截多播包，导致 mDNS 不可见。需在路由器管理界面或手机「网络加速」设置中关闭多播过滤，或直接通过 IP 连接（参见故障排查）。

---

## 4. 构建并启动 macOS Server

```bash
cd /path/to/anyMic/server

# 开发模式（有调试日志输出）
cargo run -p anymic-app

# 发布模式（性能最优）
cargo run -p anymic-app --release
```

启动成功后，Tauri 窗口弹出，状态栏显示 UDP/TCP 已监听，mDNS 已注册。  
日志示例：

```
[anymic] UDP listener bound :50127
[anymic] TCP listener bound :50128
[anymic] mDNS registered: _anymic._udp.local. port=50127
```

---

## 5. 构建并安装 Android APK

### 5.1 Debug APK（开发测试）

```bash
cd /path/to/anyMic/android
./gradlew installDebug
```

### 5.2 含 instrumented 测试的 APK（完整测试）

```bash
./gradlew installDebug installDebugAndroidTest
```

等价命令（通过 Makefile）：

```bash
make test-android
```

---

## 6. 首次连接

1. 确认 Mac server 已启动（Tauri 窗口可见）
2. 手机打开 anyMic app
3. 点击 **「Discover」**，等待约 1–3 秒，列表中出现 Mac 服务器
4. 选中服务器，点击 **「Connect」**
5. 连接建立后，app 跳转到 **StatsScreen**，显示 RTT 和包统计
6. 在 Mac 上，打开任意应用（Zoom、微信、QuickTime Player 等）
7. 在该应用的音频输入设置中，选择 **「BlackHole 2ch」** 作为麦克风

> 提示：QuickTime → 文件 → 新建音频录制 → 点击录制按钮右侧下拉箭头 → 选择 BlackHole 2ch，即可快速验证音频链路。

---

## 7. 运行测试

```bash
# Rust 单元测试 + property-based 测试（35+ 用例）
make test-server

# Android instrumented 测试（需 USB 连接真机，17+ 用例）
make test-android

# Python 端到端声学链路测试（chirp 延迟 + 丢包率）
make test-e2e

# PESQ 语音质量评分 + 频谱分析
make test-quality
```

---

## 8. 故障排查

### mDNS 不可见（Discover 列表为空）

**可能原因**：路由器/防火墙拦截多播包（`224.0.0.251:5353`）。

**排查步骤**：

```bash
# Mac 端验证 mDNS 注册
dns-sd -B _anymic._udp local
# 应输出服务名称

# 或使用 avahi-browse（Linux）
avahi-browse -r _anymic._udp
```

**临时绕过**：在手机端手动填写 Mac IP 和端口（50127/50128）直连。

---

### 真机看不到 server（连接超时）

**验证 UDP 端口连通性**：

```bash
# 通过 ADB shell 在手机上测试 UDP
adb shell nc -zu <mac-ip> 50127

# 验证 TCP 端口
adb shell nc -z <mac-ip> 50128
```

**检查 Mac 防火墙**：系统设置 → 网络 → 防火墙，确认 anymic-app 被允许接受传入连接。

---

### MIUI / 小米设备 UDP 沉默丢包

小米部分 ROM 会静默丢弃 UDP 多播包。anyMic 已内置 TCP keepalive socket 处理此问题，如仍有问题，尝试：

1. 关闭手机「网络加速」功能
2. 在路由器管理界面关闭 IGMP snooping
3. 使用手动 IP 直连模式

---

### macOS Apple Silicon 公证未通过

首次运行从源码编译的 app 可能被 Gatekeeper 拦截。

```bash
# 右键点击 app → 打开，然后点击「打开」
# 或通过命令行跳过隔离标记
xattr -d com.apple.quarantine /path/to/anymic-app.app
```

---

### BlackHole 设备索引找错

如果 ffmpeg 或其他应用找不到 BlackHole，先确认实际设备索引：

```bash
ffmpeg -list_devices true -f avfoundation -i "" 2>&1
# 输出所有音频设备及其索引，找到 BlackHole 2ch 对应编号
```

在 Mac 系统设置 → 声音中，确认 BlackHole 2ch 可见。如不可见，重新安装：

```bash
brew uninstall blackhole-2ch && brew install blackhole-2ch
```

---

### 音频卡顿或噪音

- 确认 Wi-Fi 信号良好（推荐 5GHz，避免 2.4GHz 拥挤信道）
- 检查 server 日志中的 PLC 和 stall 计数（应为 0）
- 查看 StatsScreen 中的 jitter p95（正常 < 20ms）

---

## Windows

### 系统要求
- Windows 10 (1809+) / Windows 11 — **必须 x64 架构**
- 同 Wi-Fi 网段 + 手机端

> ⚠️ **ARM Windows 不支持**
>
> VB-CABLE 是内核态驱动,VB-Audio 没有发布 ARM64 版本,所以 ARM Windows
> (Surface X、Mac 上的 Parallels ARM Windows VM 等)装 VB-CABLE 后系统
> 设备列表看不到 CABLE Input/Output,anyMic 启动会报
> "virtual audio device not found"。这是 VB-CABLE 的限制,不是 anyMic
> 的 bug —— 任何依赖虚拟音频回环驱动的 Windows 麦克风方案在 ARM 上都
> 行不通,除非你愿意自己写并签 ARM 内核驱动(本项目的零成本路线明确
> 排除这条)。
>
> 如果你只有 ARM Mac:**直接用 macOS server**(BlackHole 是 ARM 原生
> 支持的,Apple Silicon 上完美工作),不需要走 Windows VM 这一步。

### 安装 VB-CABLE
1. 下载 https://vb-audio.com/Cable/
2. 解压后**以管理员身份运行** `VBCABLE_Setup_x64.exe`
3. 安装完重启系统（必需）
4. 验证：声音设置 → 录制设备 → 应该看到 "CABLE Output"

### 启动 anyMic
- 解压 anyMic 安装包，运行 `anymic-app.exe`
- 系统托盘出现 anyMic 图标
- 主窗口显示本机 IP / 端口 / 客户端连接状态
- Windows 防火墙首次会弹提示，允许"专用网络"访问

### 在系统应用里使用
- Zoom / Teams / OBS / 微信：输入设备选 **CABLE Output (VB-Audio Virtual Cable)**
- Windows 系统设置 → 系统 → 声音 → 输入

### 故障排查
- "Virtual device not found" → 检查顺序:
  1. **是 ARM Windows 吗?** ARM 架构装不上 VB-CABLE,见上文 ARM 警告框
  2. 系统设置 → 声音 → 输入 → 看不到 "CABLE Output" → VB-CABLE 没装好,以管理员身份重装并**必须重启**
  3. 看到 "CABLE Output" 但 anyMic 还报错 → 设备名跟我代码 hardcode 的 "CABLE Input" 不匹配,把实际名字告诉我我调
- mDNS 不工作 → 装 Bonjour Print Services for Windows,或在 Android 端用手动 IP
- Windows 防火墙挡 50127/50128 → 控制面板放行 anymic-app

---

## 9. 卸载

```bash
# 卸载 Android app
adb uninstall com.anymic.app

# 删除 Rust 编译产物
cd /path/to/anyMic/server
cargo clean

# 卸载 BlackHole
brew uninstall blackhole-2ch

# 删除 Python 虚拟环境
rm -rf /path/to/anyMic/tests/venv
```
