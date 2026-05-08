# anyMic Protocol Specification — Version 1

**Status:** Draft  
**Revision:** 1.0.0  
**Date:** 2026-05-08  
**License:** MIT  

---

## Table of Contents

1. [Protocol Overview](#1-protocol-overview)
2. [Data Channel — Binary Format](#2-data-channel--binary-format)
3. [Control Channel — Protobuf over TCP](#3-control-channel--protobuf-over-tcp)
4. [mDNS Service Discovery](#4-mdns-service-discovery)
5. [Version Negotiation](#5-version-negotiation)
6. [Loss Recovery and Reconnection](#6-loss-recovery-and-reconnection)
7. [Security Considerations](#7-security-considerations)
8. [Error Code Reference](#8-error-code-reference)
9. [Comparison with Existing Protocols](#9-comparison-with-existing-protocols)

---

## 1. Protocol Overview

### 1.1 Goals

anyMic is designed to stream microphone audio from a mobile device to a desktop computer on the same local-area network with the lowest possible latency and implementation complexity. The protocol must:

- Deliver audio end-to-end in under 30 ms (encoding + network + jitter buffer) on an uncongested Wi-Fi network.
- Recover gracefully from transient packet loss without stalling the audio stream.
- Allow a new client implementation to be written in a weekend from this document alone.
- Stay within the capabilities of a standard Android or iOS audio HAL: 48 kHz, mono, 16-bit PCM input, Opus encoding in real time.

### 1.2 Two-Channel Architecture

anyMic uses two separate transport channels that serve fundamentally different purposes.

**Data channel — UDP**

Audio is latency-sensitive and tolerates occasional loss better than it tolerates delay. UDP provides the lowest-latency path with no per-packet retransmission overhead. A 5 ms frame that arrives 6 ms late is useless to the audio pipeline and should be discarded; TCP's retransmission would make it arrive even later and would stall all subsequent frames. The data channel therefore uses UDP exclusively.

**Control channel — TCP**

Session setup, parameter negotiation, heartbeat, and disconnection are low-frequency events where correctness and ordering matter. These messages are small, infrequent, and must not be lost or reordered. TCP provides reliable, ordered delivery without the need for a custom reliability layer. Protobuf encoding makes the schema self-documenting, versioned, and easy to evolve.

### 1.3 Why Not Raw RTP?

The 12-byte anyMic header is deliberately inspired by RTP (RFC 3550) but is not a valid RTP packet for the following reasons:

| Concern | RTP approach | anyMic approach |
|---------|-------------|-----------------|
| Header size | 12 bytes minimum, often 16–24 with extensions | Fixed 12 bytes, no extensions |
| Magic byte | No magic; demux relies on port assignment | `0xA1` magic allows future port sharing |
| SSRC | 32-bit SSRC, full RTCP required | 16-bit ssrc16, no RTCP overhead |
| Payload type | 7-bit PT with RTCP SDP negotiation | 1-byte PT negotiated in HelloAck |
| Multiplexing | Requires RTCP and RTSP or SDP signalling | Self-contained: TCP handshake covers all |
| Library dependency | libortp or WebRTC stack | Plain UDP socket + 12-byte struct |

A full RTP stack adds roughly 50 kLOC of dependency. anyMic's data header can be serialised/deserialised with a single 12-byte struct copy. The trade-off is that anyMic is not compatible with standard RTP sinks (e.g., SIP phones), but that is not a goal of this project.

### 1.4 Port Assignments

| Port | Protocol | Purpose |
|------|----------|---------|
| 50127 | UDP | Audio data stream |
| 50128 | TCP | Control channel (Protobuf) |

Both ports are registered in the mDNS TXT record. Neither port requires elevated privilege on any supported OS. Servers MUST listen on both ports for the duration of the application's lifetime, regardless of whether a client is connected.

---

## 2. Data Channel — Binary Format

### 2.1 Byte Order

All multi-byte integer fields in the anyMic header are encoded in **big-endian (network byte order)**. This applies to `seq`, `timestamp`, and `ssrc16`. Payload data (Opus frames) is an opaque byte string; byte order within the payload is defined by the Opus specification (RFC 6716) and is not relevant to the anyMic header parser.

### 2.2 Header Layout

```
 0               1               2               3
 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     magic     |    version    |     flags     |  payload_type |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|             seq (u16 BE)      |           timestamp (u32 BE)  |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     timestamp (u32 BE, cont.) |           ssrc16 (u16 BE)     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         payload ...                            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

Total header length: **12 bytes**. Payload begins at byte offset 12.

### 2.3 Field Descriptions

#### `magic` — offset 0, 1 byte

Fixed value `0xA1`. A receiver that sees any other value in this position MUST discard the packet silently (no ICMP port-unreachable generation). The magic byte allows future sharing of the UDP port for different packet types and guards against stray UDP traffic being parsed as audio data.

**Legal values:** `0xA1` only.  
**On invalid value:** Discard packet; increment `invalid_magic` counter.

#### `version` — offset 1, 1 byte

Protocol version encoded as two nibbles:
- **High nibble (bits 7–4):** Major version. Breaking changes increment major. Two peers with different major versions are incompatible.
- **Low nibble (bits 3–0):** Minor version. Backward-compatible additions increment minor.

For protocol v1.0 the byte value is `0x10` (major=1, minor=0).

**Legal values:** Any byte where the high nibble is non-zero.  
**On invalid value:** If the major version is unsupported, send `ErrorMsg{VERSION_MISMATCH}` on the control channel, then send `Disconnect`, then close TCP. Discard the offending UDP packet.

Encoding pseudo-code:
```
version_byte = (major & 0x0F) << 4 | (minor & 0x0F)
```

Decoding pseudo-code:
```
major = (version_byte >> 4) & 0x0F
minor = version_byte & 0x0F
```

#### `flags` — offset 2, 1 byte

Bit field. Bit 0 is the least-significant bit.

| Bit | Name | Description |
|-----|------|-------------|
| 0 | `marker` | Set on the first packet of a new utterance (speech onset after silence). Receivers MAY use this to reset jitter buffer timing. |
| 1 | `dtx` | Discontinuous Transmission. When set, the sender is in a silence period and this packet contains a Comfort Noise payload or is a keep-alive empty frame. Receivers MUST still process the packet for sequence number continuity. |
| 2 | `encrypted` | Reserved for future DTLS-SRTP. When set, the payload is encrypted and the receiver must use the negotiated key material. MUST be `0` in v1.0; receivers that see `1` when no key material was negotiated MUST discard the packet. |
| 3–7 | reserved | MUST be sent as `0`. Receivers MUST ignore these bits (do not reject packets based on reserved bits). |

#### `payload_type` — offset 3, 1 byte

Identifies the codec and format of the payload bytes.

| Value | Name | Description |
|-------|------|-------------|
| `0x00` | Reserved | Invalid; receiver MUST discard. |
| `0x01` | `OPUS_48K_MONO` | Opus frame, 48 kHz sample rate, mono, 5 ms frame (240 samples). |
| `0x02` | `PCM16_RAW` | Raw signed 16-bit PCM, little-endian, 48 kHz, mono. Intended for debugging only; not permitted in production sessions unless explicitly negotiated. |
| `0x03–0xFF` | Reserved | Receiver MUST discard packets with unknown payload_type values. |

The actual payload type in use is negotiated during the TCP handshake (`HelloAck.chosen_codec`). A receiver that receives a payload_type that does not match the negotiated codec MUST discard the packet and MAY log a warning.

#### `seq` — offset 4, 2 bytes (u16 big-endian)

Unsigned 16-bit sequence number. Incremented by 1 for each UDP packet sent. Wraps from `0xFFFF` to `0x0000` (modular arithmetic).

Loss detection uses wrap-aware comparison: given last received sequence `L` and incoming sequence `S`, the packet is considered in-order if `(S - L) mod 65536 < 32768`. Packets where `(S - L) mod 65536 >= 32768` are considered late/reordered and SHOULD be discarded.

**First packet:** Sequence number starts at a random value in `[0, 65535]` to reduce the chance of conflicts with stale packets from a previous session.

#### `timestamp` — offset 6, 4 bytes (u32 big-endian)

Sample clock timestamp. The timestamp unit is **samples at 48 kHz** (one tick = 1/48000 s ≈ 20.83 µs). For a 5 ms frame the timestamp advances by 240 samples per packet.

**First packet:** Timestamp starts at a random value in `[0, 2^32 - 1]`. Receivers MUST NOT assume that the first packet's timestamp is zero.

**Wrap handling:** Timestamp wraps from `0xFFFFFFFF` to `0x00000000` after approximately 89,478 seconds (≈ 24.8 hours) of continuous streaming. Receivers MUST handle wrap-around with the same modular arithmetic used for `seq`.

**Use by receiver:** The timestamp is used by the jitter buffer to reconstruct the correct audio frame timeline. If two packets arrive with the same timestamp they are duplicates; the receiver SHOULD keep the first and discard the second.

#### `ssrc16` — offset 10, 2 bytes (u16 big-endian)

Lower 16 bits of the 32-bit SSRC assigned by the server in `HelloAck.ssrc`. Used by the server to demultiplex packets from multiple clients when the server supports multiple simultaneous connections. Clients MUST use the exact value received in HelloAck and MUST NOT change it during a session.

**On value mismatch:** The server SHOULD discard packets whose ssrc16 does not match any active session and SHOULD NOT log at error level (stray packets are expected on a LAN).

### 2.4 Packet Size Constraints

To avoid IP fragmentation, every UDP packet (IP payload) MUST be kept below **1200 bytes**. This leaves headroom for IP (20 B) and UDP (8 B) headers within the standard 1500-byte Ethernet MTU and also satisfies the conservative MTU assumed by QUIC and WebRTC.

| Component | Size |
|-----------|------|
| anyMic header | 12 B |
| Opus 5 ms frame (typical) | 25–60 B |
| Opus 5 ms frame (maximum at 256 kbps) | ~160 B |
| **Total typical** | **37–72 B** |
| **Total maximum safe** | < 1200 B |

No anyMic payload type produces frames near the 1200-byte limit with the negotiated 5 ms frame size. Implementations MUST nevertheless validate that `payload length = packet_length - 12 < 1188` and discard oversized packets.

### 2.5 Wire Example — Annotated Hex Dump

The following is a complete anyMic UDP payload for a single Opus 5 ms frame (30-byte Opus payload for illustration).

```
Offset  Hex bytes         Field              Value / Notes
------  ----------------  -----------------  ------------------------------------
  0     A1                magic              0xA1 — anyMic v1 identifier
  1     10                version            0x10 → major=1, minor=0
  2     01                flags              bit0=1 marker (speech onset)
                                             bit1=0 DTX off
                                             bit2=0 not encrypted
                                             bits3-7 = 0 reserved
  3     01                payload_type       0x01 = OPUS_48K_MONO
  4-5   00 2C             seq                0x002C = 44 (44th packet)
  6-9   3B 9A CA 00       timestamp          0x3B9ACA00 = 1 000 000 000 samples
                                             = 1 000 000 000 / 48000 s
                                             ≈ 20833.3 s from session epoch
 10-11  F3 7A             ssrc16             0xF37A = session SSRC low 16 bits
 12-41  FC FF FE 4A 8C    payload (30 B)     Opus frame bytes (opaque)
        D1 0E 37 A2 5B
        C4 89 3F 60 11
        2D 9E 47 B8 73
        05 AA 1C E6 98
        4F 27 86 CC 31
        7E 59 DA

Total: 42 bytes (12 header + 30 payload)
```

**Bandwidth calculation:**
- 42 bytes/packet × 200 packets/s = 8400 bytes/s = 67.2 kbps
- UDP+IP overhead: (8+20) bytes/packet × 200 = 5600 bytes/s = 44.8 kbps
- **Total on wire: ≈ 112 kbps** (well within typical Wi-Fi capacity)

---

## 3. Control Channel — Protobuf over TCP

### 3.1 Transport Framing

Each Protobuf message is preceded by a 4-byte big-endian unsigned integer indicating the byte length of the serialised Protobuf payload that follows. Receivers MUST read exactly `length` bytes and then deserialise the Protobuf message.

```
+-------------------+----------------------------------+
|  length (u32 BE)  |   Protobuf serialised message    |
|     4 bytes       |   `length` bytes                 |
+-------------------+----------------------------------+
```

Maximum allowed message length: **65535 bytes**. Receivers that see a length value above this threshold MUST close the TCP connection immediately without reading further data.

The Protobuf messages are wrapped in `ClientMsg` (client → server) or `ServerMsg` (server → client) `oneof` envelopes to allow a single TCP stream to carry all message types without an additional type tag. See `proto/anymic.proto` for the full schema.

### 3.2 Message Catalogue

| Message | Direction | Description |
|---------|-----------|-------------|
| `Hello` | C → S | Session initiation, version and codec advertisement |
| `HelloAck` | S → C | Session parameters, SSRC, negotiated codec |
| `Ready` | C → S | Client confirms UDP socket open, streaming may begin |
| `Stats` | C → S | Periodic heartbeat, QoS metrics |
| `Pong` | S → C | Response to Stats, server timestamp echo |
| `Disconnect` | C ↔ S | Graceful session termination |
| `ErrorMsg` | C ↔ S | Protocol error notification |

### 3.3 State Machine

```
Client                              Server
  │                                   │
  │──── TCP SYN ──────────────────→   │  (TCP connect to port 50128)
  │←─── TCP SYN-ACK ────────────────  │
  │                                   │
  │  [STATE: CONNECTING]              │
  │                                   │
  │──── Hello ────────────────────→   │
  │                                   │  (validate version, pick codec)
  │←─── HelloAck ──────────────────   │
  │                                   │
  │  [STATE: HANDSHAKING]             │
  │                                   │
  │──── Ready ────────────────────→   │
  │                                   │
  │  [STATE: STREAMING]               │  [STATE: STREAMING]
  │                                   │
  │  ← UDP audio flows →              │
  │                                   │
  │──── Stats (every 1 s) ────────→   │
  │←─── Pong ──────────────────────   │
  │                                   │
  │──── Disconnect ───────────────→   │
  │  [TCP close]                      │  [TCP close]
```

### 3.4 Sequence Diagrams

#### 3.4.1 Initial Handshake (Success)

```
Client                              Server
  │                                   │
  │──[ Hello ]────────────────────→   │
  │  client_id: "uuid-abc..."         │
  │  display_name: "Pixel 7"          │
  │  os: ANDROID                      │
  │  min_version_major: 1             │
  │  max_version_major: 1             │
  │  codec_caps: [OPUS_48K_MONO]      │
  │  sample_rates: [48000]            │
  │  resume_session_id: ""            │
  │                                   │
  │←─[ HelloAck ]──────────────────   │
  │  session_id: "uuid-xyz..."        │
  │  ssrc: 0xF37ABCDE                 │
  │  chosen_codec: OPUS_48K_MONO      │
  │  sample_rate: 48000               │
  │  frame_ms: 5                      │
  │  negotiated_version: 0x10         │
  │  udp_port: 50127                  │
  │                                   │
  │  (client opens UDP socket)        │
  │                                   │
  │──[ Ready ]────────────────────→   │
  │                                   │
  │  [STREAMING: UDP begins]          │
```

#### 3.4.2 Heartbeat Exchange

```
Client                              Server
  │                                   │
  │──[ Stats ]────────────────────→   │  t=0
  │  rtt_ms: 4                        │
  │  packets_sent: 1000               │
  │  packets_lost: 2                  │
  │  jitter_ms: 1                     │
  │  battery_pct: 72                  │
  │  input_level_dbfs: -18            │
  │  client_ts_ms: 5000               │
  │                                   │
  │←─[ Pong ]──────────────────────   │  t≈0
  │  server_ts_ms: 1715174400123      │
  │  echoed_client_ts_ms: 5000        │
  │                                   │
  │  (next Stats at t=1000 ms)        │
  │──[ Stats ]────────────────────→   │  t=1000
  │←─[ Pong ]──────────────────────   │
  │                                   │
  │  (3 consecutive Stats without     │
  │   Pong → server-side timeout)     │
```

#### 3.4.3 Reconnect within Fast Window (≤ 3 s)

```
Client                              Server
  │                                   │
  │  [Network interruption]           │
  │  [TCP broken]                     │  [session kept alive for 3 s]
  │                                   │
  │──[ TCP reconnect ]────────────→   │
  │──[ Hello ]────────────────────→   │
  │  resume_session_id: "uuid-xyz..."│
  │                                   │  (validates session still active)
  │←─[ HelloAck ]──────────────────   │
  │  session_id: "uuid-xyz..."  ←same │  (same session, same SSRC)
  │  ssrc: 0xF37ABCDE  ←same         │
  │                                   │
  │──[ Ready ]────────────────────→   │
  │                                   │
  │  [STREAMING: UDP continues]       │  (seq/timestamp continue from
  │                                   │   where they left off)
```

#### 3.4.4 Server Rejection

```
Client                              Server
  │                                   │
  │──[ Hello ]────────────────────→   │
  │  min_version_major: 2             │  (server only supports major 1)
  │  max_version_major: 2             │
  │                                   │
  │←─[ ErrorMsg ]──────────────────   │
  │  code: VERSION_MISMATCH           │
  │  message: "Server supports v1.x   │
  │            only"                  │
  │  detail: "version"                │
  │                                   │
  │←─[ Disconnect ]────────────────   │
  │  reason: "version incompatible"   │
  │                                   │
  │  [Server closes TCP]              │
```

Another rejection example — too many clients:

```
Client                              Server
  │                                   │
  │──[ Hello ]────────────────────→   │  (max 4 clients already connected)
  │                                   │
  │←─[ ErrorMsg ]──────────────────   │
  │  code: TOO_MANY_CLIENTS           │
  │  message: "Maximum 4 clients"     │
  │                                   │
  │←─[ Disconnect ]────────────────   │
  │  [Server closes TCP]              │
```

#### 3.4.5 Client-Initiated Graceful Disconnect

```
Client                              Server
  │                                   │
  │──[ Disconnect ]───────────────→   │
  │  reason: "user stopped app"       │
  │                                   │
  │  [Client stops UDP]               │  [Server stops accepting UDP
  │  [Client closes TCP]              │   from this ssrc16]
  │                                   │  [Server releases session]
```

### 3.5 Timing Requirements

| Event | Requirement |
|-------|-------------|
| Time from TCP connect to Hello sent | ≤ 5 s (server closes TCP if no Hello received) |
| Time from Hello to HelloAck | ≤ 2 s (server processing budget) |
| Time from HelloAck to Ready | ≤ 3 s (client budget to open UDP socket) |
| Stats interval | 1000 ms ± 100 ms |
| Stats timeout (server-side disconnect) | 3 consecutive intervals without Stats → Disconnect |
| Disconnect TCP close deadline | 500 ms after sending Disconnect |

---

## 4. mDNS Service Discovery

### 4.1 Service Type

anyMic servers advertise themselves using DNS-SD (RFC 6763) over mDNS (RFC 6762):

```
Service type: _anymic._udp.local.
```

Despite the service type using `_udp`, the mDNS advertisement covers both ports. The `_udp` designation refers to the primary data channel. The TCP control port is carried in the TXT record.

### 4.2 Port

| Record type | Value | Meaning |
|-------------|-------|---------|
| SRV | 50127 | UDP data port (primary) |
| TXT `ctl` | 50128 | TCP control port |

### 4.3 TXT Record Fields

The TXT record MUST contain the following key=value pairs. Keys are case-insensitive on reception but SHOULD be sent lowercase.

| Key | Value format | Required | Description |
|-----|-------------|----------|-------------|
| `v` | Integer | Yes | anyMic protocol major version. `v=1` for this specification. |
| `name` | UTF-8 string, max 63 bytes | Yes | Human-readable server display name (e.g., `name=MacBook Pro`). |
| `os` | `mac`, `win`, `linux` | Yes | Server operating system. |
| `codec` | Comma-separated codec list | Yes | Supported codecs (e.g., `codec=opus48`). `opus48` means OPUS_48K_MONO. |
| `fid` | 8 hex digits | Yes | Fingerprint: first 8 hex digits of SHA-256(machine UUID). Used to disambiguate servers with identical names. |
| `ctl` | Integer port number | Yes | TCP control channel port. Default `50128`. |
| `max` | Integer | No | Maximum simultaneous clients (e.g., `max=4`). Omit if unlimited. |

**Example TXT record:**

```
v=1
name=MacBook Pro (M3)
os=mac
codec=opus48,pcm16
fid=3a7f9c2e
ctl=50128
max=4
```

### 4.4 Registration (Server Side)

1. On application start, bind UDP port 50127 and TCP port 50128.
2. Register the mDNS service using the system mDNS library (Bonjour on macOS, Avahi on Linux, NSD on Windows).
3. Set TTL to 4500 seconds; re-announce every 1000 seconds.
4. On application quit, send a "goodbye" announcement (TTL=0) to allow clients to remove the record immediately.

On macOS, the registration uses `DNSServiceRegister()` with service type `_anymic._udp` and the TXT record constructed as above.

### 4.5 Discovery (Client Side)

1. Start browsing for `_anymic._udp.local.` using the system mDNS browser.
2. For each discovered instance, resolve the SRV record to get hostname and port, then resolve the A/AAAA record to get IP addresses.
3. Parse the TXT record; discard servers with `v` field incompatible with the client's supported major version.
4. Present the list of discovered servers to the user, sorted by `fid` for stable ordering.
5. When the user selects a server, establish a TCP connection to the resolved IP address and TXT `ctl` port.

### 4.6 Manual IP Fallback

If mDNS browsing returns no results within 5 seconds, or if mDNS is unavailable (some enterprise Wi-Fi networks block mDNS), the client MUST offer a manual entry screen where the user can input:

- IP address or hostname
- UDP port (default 50127)
- TCP control port (default 50128)

The connection procedure after manual entry is identical to mDNS-discovered connections.

---

## 5. Version Negotiation

### 5.1 Compatibility Model

anyMic uses a two-level version scheme:

- **Major version:** Incompatible changes. Two peers with different major versions MUST NOT communicate.
- **Minor version:** Backward-compatible additions only. A peer running v1.2 can communicate with a peer running v1.0 by using only the features available in v1.0.

### 5.2 Negotiation Procedure

The client advertises a range `[min_version_major, max_version_major]` in `Hello`. The server picks its own supported version from this range. If the intersection is empty, the server sends `ErrorMsg{VERSION_MISMATCH}`.

```
Server logic:

  server_major = 1  // this server's major version

  if client.min_version_major > server_major
    OR client.max_version_major < server_major:
      send ErrorMsg{code: VERSION_MISMATCH,
                    message: "Server supports major " + server_major}
      send Disconnect{reason: "version incompatible"}
      close TCP
      return

  // negotiated_version: use server's current full version
  negotiated_version = encode_version(server_major, server_minor)
  send HelloAck{negotiated_version: negotiated_version, ...}
```

The `negotiated_version` field in `HelloAck` is encoded identically to the UDP header `version` byte: `(major << 4) | minor`. A client receiving `negotiated_version = 0x12` is speaking with a v1.2 server and MUST restrict itself to v1.0 behaviour if it only knows v1.0.

### 5.3 Minor Version Compatibility Rules

The minor version is not negotiated; it is simply advertised by the server so the client knows which optional features are available. The client MUST NOT use optional fields defined in minor versions higher than the negotiated minor version.

| Minor version | Changes |
|---------------|---------|
| 0 | This specification. Base feature set. |
| 1 (future) | Encrypted payload support (`flags` bit 2 active). |
| 2 (future) | FEC field in UDP header. |

### 5.4 Version Field Encoding Pseudo-Code

```python
# Encoding
def encode_version(major: int, minor: int) -> int:
    assert 0 <= major <= 15, "major must fit in 4 bits"
    assert 0 <= minor <= 15, "minor must fit in 4 bits"
    return (major << 4) | (minor & 0x0F)

# Decoding
def decode_version(byte: int) -> tuple[int, int]:
    major = (byte >> 4) & 0x0F
    minor = byte & 0x0F
    return major, minor

# Examples
assert encode_version(1, 0) == 0x10
assert decode_version(0x10) == (1, 0)
assert decode_version(0x21) == (2, 1)
```

---

## 6. Loss Recovery and Reconnection

### 6.1 UDP Retransmission Policy

anyMic does **not** retransmit lost UDP packets. Rationale:

1. **Latency budget:** A 5 ms frame must be played within ~20 ms of its capture time to maintain imperceptible latency. By the time a loss is detected and a retransmission arrives, the deadline has already passed.
2. **Opus PLC:** The Opus decoder contains a built-in Packet Loss Concealment (PLC) mechanism. A single lost frame is extrapolated from the preceding frame's waveform characteristics; typical PLC quality is indistinguishable from a real frame at loss rates below 5%.
3. **DTX cooperation:** When DTX is active, silence frames are omitted. The receiver treats gaps in DTX periods as intentional silence, not loss.

**What implementations MUST do instead:**

- Monitor `seq` for gaps. Gaps of 1–2 packets: call `opus_decode()` with a null input to invoke PLC.
- Gaps of 3–10 packets: invoke PLC for each missing frame; reset jitter buffer timing if the gap spans more than 15 ms.
- Gaps of > 50 consecutive frames: declare a Stall (see §6.4).

### 6.2 Jitter Buffer

The receiver maintains an adaptive jitter buffer to smooth out network arrival jitter.

**Target depth calculation:**

```
target_depth_frames = max(1, ceil(P95_jitter_ms / frame_ms))
```

Where:
- `P95_jitter_ms` is the 95th percentile of the inter-packet arrival jitter over the last 200 frames (1-second window at 5 ms frames).
- `frame_ms` is 5 ms.
- The result is the number of frames to hold in the buffer before starting playout.

**Smoothing:** The target depth is updated with an Exponential Moving Average:

```
alpha = 1.0 / 200.0   // EMA weight
ema_jitter_ms = alpha * measured_jitter_ms + (1 - alpha) * ema_jitter_ms
target_depth = max(1, ceil(ema_jitter_ms / 5.0))
```

**Playout:** Frames are released from the buffer at the nominal 5 ms rate relative to the capture timestamp in the packet header. The jitter buffer absorbs early and late arrivals within `target_depth * frame_ms` ms.

### 6.3 Keepalive and Timeout

| Timer | Period | Action on expiry |
|-------|--------|-----------------|
| Client Stats interval | 1000 ms | Client sends Stats heartbeat |
| Server keepalive timeout | 3 × 1000 ms = 3000 ms without Stats | Server sends Disconnect, closes TCP |
| Client Pong timeout | 3 × 1000 ms = 3000 ms without Pong | Client attempts reconnect |

If the server has not received a Stats message from a client in 3 seconds, the server MUST:
1. Send `Disconnect{reason: "keepalive timeout"}`.
2. Close the TCP connection.
3. Release the SSRC and session slot.
4. Stop accepting UDP packets from the client's ssrc16.

### 6.4 Stall and Reconnection

A **Stall** is declared on the receiving side when more than 50 consecutive expected UDP frames are absent (250 ms of silence with no DTX flag).

A **Disconnect** is triggered when more than 500 consecutive frames are absent (2.5 s).

**Reconnection algorithm:**

```
on_disconnect():
    wait_ms = 0
    attempt = 0
    while not connected:
        attempt += 1
        tcp_connect(server_ip, control_port)
        if elapsed_since_disconnect <= 3000ms:
            // Fast reconnect path: reuse session
            send Hello{resume_session_id: saved_session_id, ...}
        else if elapsed_since_disconnect <= 5000ms:
            // Slow reconnect path: session may still exist
            send Hello{resume_session_id: saved_session_id, ...}
            // Server will respond with same session or new one
        else:
            // Full reconnect: session certainly expired
            send Hello{resume_session_id: "", ...}
        
        if HelloAck.session_id == saved_session_id:
            // Resume: continue seq and timestamp from last values
        else:
            // New session: reset seq and timestamp to new random values
        
        back_off = min(30000ms, 1000ms * 2^attempt)
        if not connected: wait(back_off)
```

**Session lifetime on server:**

| Elapsed since TCP close | Server behaviour |
|------------------------|-----------------|
| 0 – 3 s | Session kept alive; fast reconnect accepted |
| 3 – 5 s | Session kept alive; reconnect accepted but de-prioritised |
| > 5 s | Session expired; always new session |

---

## 7. Security Considerations

### 7.1 Threat Model

anyMic v1 is designed for use on **trusted local-area networks** (home or office Wi-Fi). It provides:

- **No authentication:** Any device on the LAN can connect as a client.
- **No payload encryption:** Audio data is transmitted in cleartext over UDP.
- **No integrity protection:** Packet fields can be spoofed by any LAN participant.

These are intentional design choices to minimise implementation complexity for the initial version. The threat model assumes that the LAN itself is trusted and access-controlled (Wi-Fi password, router firewall rules).

### 7.2 Public Network Risk

**Do not use anyMic v1 over untrusted networks.** Risks include:

- **Eavesdropping:** Any device on the same network can capture and replay the UDP audio stream.
- **Session hijacking:** An attacker knowing the SSRC can inject spoofed packets with the correct ssrc16, corrupting the audio stream on the server.
- **Denial of service:** An attacker can flood the UDP port with packets bearing a valid magic byte and ssrc16, consuming server processing resources.
- **Information disclosure:** The mDNS advertisement reveals the server hostname and OS type to all LAN participants.

### 7.3 Mitigations Available in v1

- **Port binding:** The server only accepts UDP packets when a client session is active. Servers SHOULD drop packets from IP addresses not matching the current session's TCP source IP.
- **Magic byte:** The `0xA1` magic reduces accidental processing of stray UDP traffic.
- **ssrc16 validation:** The server validates that incoming ssrc16 matches an active session.
- **Rate limiting:** Servers SHOULD implement a simple token-bucket rate limiter: no more than 400 packets/s per source IP (2× the nominal rate). Packets exceeding this rate SHOULD be silently dropped.

### 7.4 Reserved Encryption Flag (flags bit 2)

The `encrypted` bit (bit 2 of the `flags` field) is reserved for a future DTLS-SRTP extension. When a future minor version enables encryption:

1. The TCP handshake will include a DTLS-SRTP key exchange step between HelloAck and Ready.
2. The server sets `flags` bit 2 = 1 in all packets it expects to receive encrypted.
3. The payload will be the SRTP ciphertext; the 12-byte anyMic header remains plaintext.
4. Receivers seeing bit 2 = 1 without a negotiated key MUST discard the packet.

In v1.0, bit 2 MUST always be 0. Any implementation that receives bit 2 = 1 when no key material was negotiated MUST silently discard the packet.

### 7.5 Future Hardening Roadmap

| Feature | Target version |
|---------|---------------|
| DTLS-SRTP payload encryption | v1.1 |
| HMAC-based packet authentication | v1.1 |
| Client certificate pinning | v2.0 |
| Mutual TLS on control channel | v2.0 |

---

## 8. Error Code Reference

Error codes are returned in `ErrorMsg.code` (enum `ErrorCode` in `proto/anymic.proto`). Numeric values are stable across protocol versions and MUST NOT be reused.

| Code | Name | Numeric | Direction | Description | Client action |
|------|------|---------|-----------|-------------|--------------|
| 0 | `ERROR_UNSPECIFIED` | 0 | Both | Default / unknown error. | Display generic error to user; reconnect after back-off. |
| 1 | `VERSION_MISMATCH` | 1 | S → C | Client requested a major version the server does not support. `detail` contains the supported major version range. | Show "incompatible server version" UI. Do not retry with same server. |
| 2 | `CODEC_UNSUPPORTED` | 2 | S → C | None of the codecs in `Hello.codec_caps` is supported by the server. | Show "codec not supported" UI. |
| 3 | `SESSION_EXPIRED` | 3 | S → C | The `resume_session_id` supplied in Hello has expired (> 5 s). Server will start a new session. | Use new session_id from HelloAck; reset seq and timestamp. |
| 4 | `TOO_MANY_CLIENTS` | 4 | S → C | Server has reached its maximum simultaneous client count. | Display "server full" UI. Retry with exponential back-off. |
| 5 | `AUTH_FAILED` | 5 | S → C | Reserved for future authentication. Not used in v1.0. | — |
| 6 | `SERVER_BUSY` | 6 | S → C | Server is temporarily overloaded (CPU, buffer full). | Retry after 5–30 s random back-off. |
| 7 | `INVALID_PACKET` | 7 | Both | A received message was structurally invalid, contained illegal field values, or violated the state machine. `detail` describes which field. | Log error; if from server, display error to user. If from client, server closes TCP. |
| 8 | `UNEXPECTED_MESSAGE` | 8 | S → C | Client sent a message out of sequence (e.g., Ready before HelloAck, or a second Hello while streaming). | Log and reconnect from scratch. |
| 9 | `KEEPALIVE_TIMEOUT` | 9 | S → C | Server did not receive Stats for more than 3 consecutive intervals. Server is terminating the session. | Reconnect using fast-reconnect path. |

**Sending ErrorMsg:** After sending `ErrorMsg`, the sender SHOULD immediately send `Disconnect{reason: ...}` and close the TCP connection within 500 ms. Receivers of `ErrorMsg` MUST NOT send further control messages; they SHOULD close their TCP connection after logging the error.

---

## 9. Comparison with Existing Protocols

### 9.1 RTP (RFC 3550)

| Aspect | RTP | anyMic |
|--------|-----|--------|
| Header size | 12 bytes (fixed) + optional extensions | 12 bytes (fixed, no extensions) |
| SSRC | 32-bit | 16-bit (sufficient for LAN) |
| Payload type | 7-bit, requires SDP/RTSP negotiation | 1-byte, negotiated in TCP Hello |
| Control protocol | RTCP (separate port, complex) | anyMic TCP (simple Protobuf) |
| Multiplexing | SSRC-based (RTCP required) | ssrc16 + TCP session |
| Interoperability | SIP phones, VoIP infrastructure | anyMic only |
| Dependency | libortp / live555 / etc. | UDP socket + 12-byte struct |
| Encryption | SRTP (RFC 3711) | Planned for v1.1 |

**Rationale for not using RTP:** RTP requires RTCP for sender/receiver reports, which adds a second UDP socket and a substantial protocol state machine. SDP negotiation requires a signalling stack (SIP, RTSP, or WebRTC). The anyMic TCP control channel is simpler and more predictable for a greenfield implementation.

### 9.2 WebRTC

| Aspect | WebRTC | anyMic |
|--------|--------|--------|
| Signalling | SDP via application-defined channel | Protobuf Hello/HelloAck |
| ICE/NAT traversal | Yes (STUN, TURN) | No (LAN only, no NAT) |
| Encryption | Mandatory DTLS-SRTP | Optional (planned v1.1) |
| Codec negotiation | SDP offer/answer | Hello.codec_caps list |
| Jitter buffer | Built into browser | Application-level (required) |
| Codec support | Opus, VP8/VP9, H.264 | Opus (audio only) |
| Dependency | Full WebRTC stack (10+ MB) | UDP + TCP + Protobuf |
| Latency | ~50–150 ms typical | ~5–20 ms (LAN, no ICE) |

**Rationale for not using WebRTC:** WebRTC is designed for peer-to-peer communication across NAT boundaries, which requires ICE negotiation with STUN/TURN servers. For a LAN-only application, this overhead is unnecessary. The mandatory DTLS-SRTP adds latency (DTLS handshake) and implementation complexity that is not justified for a trusted-LAN scenario. WebRTC libraries for Android add 10–30 MB to the APK size.

### 9.3 Opus in RTP (RFC 7587)

RFC 7587 defines how to carry Opus in RTP. anyMic is compatible at the payload level: Opus frames produced by anyMic are identical to those carried in RFC 7587 RTP packets. The difference is solely in the header wrapping and the control signalling. A future gateway bridge between anyMic and a SIP infrastructure would simply re-wrap the Opus payloads in standard RTP headers.

### 9.4 Summary

anyMic occupies a deliberate niche: a **minimal, LAN-only, single-purpose** audio streaming protocol. It prioritises:

1. **Low dependency surface:** UDP socket + TCP socket + Protobuf library. No WebRTC stack, no SIP stack, no RTCP.
2. **Low latency:** 5 ms frames, no ICE, no DTLS handshake in v1.
3. **Implementability:** A new server implementation requires only this document. The wire format can be parsed with a 12-byte struct and a single Protobuf deserialise call.
4. **Evolvability:** The version byte, reserved flags, and error code table leave room for encryption, FEC, multi-stream, and authentication without breaking v1 clients.

---

## Appendix A — Constants Reference

| Constant | Value | Notes |
|----------|-------|-------|
| `ANYMIC_MAGIC` | `0xA1` | First byte of every UDP packet |
| `VERSION_CURRENT` | `0x10` | v1.0 |
| `UDP_DATA_PORT` | `50127` | Default data port |
| `TCP_CTRL_PORT` | `50128` | Default control port |
| `FRAME_DURATION_MS` | `5` | ms per audio frame |
| `SAMPLE_RATE` | `48000` | Hz |
| `SAMPLES_PER_FRAME` | `240` | 48000 × 0.005 |
| `STATS_INTERVAL_MS` | `1000` | Client heartbeat period |
| `KEEPALIVE_TIMEOUT_MS` | `3000` | 3 × stats interval |
| `FAST_RECONNECT_WINDOW_MS` | `3000` | Same SSRC reuse window |
| `SESSION_EXPIRY_MS` | `5000` | After this, new session required |
| `STALL_THRESHOLD_FRAMES` | `50` | 250 ms of silence → stall |
| `DISCONNECT_THRESHOLD_FRAMES` | `500` | 2500 ms → reconnect |
| `MAX_PACKET_SIZE_BYTES` | `1200` | Maximum UDP payload (incl. header) |
| `MAX_TCP_MSG_SIZE_BYTES` | `65535` | Maximum Protobuf message length |
| `MDNS_SERVICE_TYPE` | `_anymic._udp.local.` | DNS-SD service type |
| `MDNS_TTL_SECONDS` | `4500` | mDNS record TTL |
| `MDNS_REANNOUNCE_INTERVAL_S` | `1000` | Periodic re-announcement |

---

## Appendix B — Changelog

| Version | Date | Summary |
|---------|------|---------|
| 1.0.0 | 2026-05-08 | Initial specification. |
