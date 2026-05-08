#!/usr/bin/env python3
"""
anyMic verification tool — sends a synthetic Opus stream over UDP.

Usage:
    python3 tools/send_opus_stream.py --host 127.0.0.1 --port 50127 \
        --duration 5 --signal sine

Requirements (auto-installed if missing):
    opuslib  (pip install opuslib)
    numpy    (pip install numpy)

The script falls back to a ffmpeg subprocess encoder if opuslib is unavailable.
"""

import argparse
import math
import socket
import struct
import sys
import time

# ── constants ────────────────────────────────────────────────────────────────

MAGIC         = 0xA1
VERSION       = 0x10   # major=1, minor=0
FLAGS         = 0x00
PAYLOAD_TYPE  = 0x01   # Opus48kMono
SAMPLE_RATE   = 48_000
FRAME_SAMPLES = 240    # 5 ms
HEADER_LEN    = 12
SSRC16        = 0x1234


# ── header packer ─────────────────────────────────────────────────────────────

def build_packet(seq, timestamp, opus_payload):
    """Assemble a complete anyMic UDP packet (12-byte header + Opus payload).

    Wire format (big-endian):
      offset 0:    magic   (u8)  = 0xA1
      offset 1:    version (u8)  = 0x10 (major=1, minor=0)
      offset 2:    flags   (u8)  = 0x00
      offset 3:    pt      (u8)  = 0x01 (Opus48kMono)
      offset 4-5:  seq     (u16 BE)
      offset 6-9:  ts      (u32 BE)
      offset 10-11: ssrc16 (u16 BE)
    """
    seq16 = seq & 0xFFFF
    ts32  = timestamp & 0xFFFFFFFF
    header = struct.pack(">BBBB", MAGIC, VERSION, FLAGS, PAYLOAD_TYPE)
    header += struct.pack(">H", seq16)
    header += struct.pack(">I", ts32)
    header += struct.pack(">H", SSRC16)
    assert len(header) == HEADER_LEN
    return header + opus_payload


# ── PCM generators ────────────────────────────────────────────────────────────

def gen_sine(num_samples: int, freq: float = 1000.0, amplitude: float = 0.3):
    """Generate mono s16 PCM for a sine wave."""
    try:
        import numpy as np
        t = np.arange(num_samples) / SAMPLE_RATE
        pcm_f = np.sin(2 * math.pi * freq * t) * amplitude
        pcm_i16 = (pcm_f * 32767).astype(np.int16)
        return pcm_i16.tobytes()
    except ImportError:
        # Pure Python fallback (slower but no deps)
        buf = bytearray(num_samples * 2)
        for i in range(num_samples):
            t = i / SAMPLE_RATE
            v = math.sin(2 * math.pi * freq * t) * amplitude
            sample = int(v * 32767)
            sample = max(-32768, min(32767, sample))
            struct.pack_into(">h", buf, i * 2, sample)
        return bytes(buf)


def gen_chirp(num_samples: int):
    """Linear chirp from 200 Hz to 3000 Hz."""
    try:
        import numpy as np
        t = np.arange(num_samples) / SAMPLE_RATE
        f0, f1 = 200.0, 3000.0
        duration = num_samples / SAMPLE_RATE
        inst_freq = f0 + (f1 - f0) * t / duration
        phase = 2 * math.pi * np.cumsum(inst_freq) / SAMPLE_RATE
        pcm_f = np.sin(phase) * 0.3
        return (pcm_f * 32767).astype(np.int16).tobytes()
    except ImportError:
        return gen_sine(num_samples, freq=440.0)


def load_wav_pcm(path: str) -> bytes:
    """Read a WAV file and return raw mono s16 @ 48 kHz PCM bytes."""
    import wave
    with wave.open(path, 'rb') as wf:
        nch     = wf.getnchannels()
        sampw   = wf.getsampwidth()
        framerate = wf.getframerate()
        nframes = wf.getnframes()
        raw = wf.readframes(nframes)

    if sampw != 2:
        sys.exit(f"WAV must be 16-bit PCM, got {sampw*8}-bit")

    # Convert to mono if stereo
    if nch == 2:
        samples = len(raw) // 2
        mono = bytearray(samples)
        for i in range(0, samples, 2):
            l = struct.unpack_from("<h", raw, i * 2)[0]
            r = struct.unpack_from("<h", raw, (i + 1) * 2)[0]
            m = (l + r) // 2
            struct.pack_into("<h", mono, i, m)
        raw = bytes(mono)
    elif nch != 1:
        sys.exit(f"WAV must be mono or stereo, got {nch} channels")

    # Resample to 48 kHz if needed (simple nearest-neighbor)
    if framerate != SAMPLE_RATE:
        try:
            import numpy as np
            samples_in  = len(raw) // 2
            samples_out = int(samples_in * SAMPLE_RATE / framerate)
            pcm_in  = np.frombuffer(raw, dtype=np.int16)
            indices = (np.arange(samples_out) * framerate / SAMPLE_RATE).astype(int)
            indices = np.clip(indices, 0, samples_in - 1)
            raw = pcm_in[indices].tobytes()
        except ImportError:
            sys.exit("numpy required for WAV resampling; pip install numpy")

    return raw


# ── Opus encoder via opuslib ──────────────────────────────────────────────────

def try_opuslib_encode(pcm_frame_bytes):
    # type: (bytes) -> bytes
    """Try to encode one frame with opuslib. Returns None if unavailable."""
    try:
        import opuslib
        # Cache encoder between calls via function attribute
        if not hasattr(try_opuslib_encode, '_enc'):
            try_opuslib_encode._enc = opuslib.Encoder(
                SAMPLE_RATE, 1, opuslib.APPLICATION_VOIP)
        enc = try_opuslib_encode._enc
        # opuslib encode takes PCM as bytes (s16le), returns bytes
        return enc.encode(pcm_frame_bytes, FRAME_SAMPLES)
    except Exception:
        return None


# ── Opus encoder via ffmpeg subprocess ────────────────────────────────────────

_ffmpeg_proc = None

def ffmpeg_encode_frame(pcm_s16le: bytes) -> bytes:
    """
    Pipe raw s16le mono 48kHz PCM to ffmpeg → ogg/opus → parse out Opus frames.

    For simplicity we use ffmpeg to transcode the entire stream at startup,
    then slice frames. This is invoked once per stream, not per frame.
    """
    raise NotImplementedError("use encode_all_frames_ffmpeg instead")


def encode_all_frames_ffmpeg(pcm_bytes: bytes) -> list:
    """Encode all PCM frames via ffmpeg to Opus, return list of raw Opus payloads."""
    import subprocess
    import tempfile
    import os

    with tempfile.NamedTemporaryFile(suffix='.raw', delete=False) as f:
        f.write(pcm_bytes)
        raw_path = f.name

    ogg_path = raw_path + '.ogg'
    try:
        subprocess.run([
            'ffmpeg', '-y', '-loglevel', 'error',
            '-f', 's16le', '-ar', str(SAMPLE_RATE), '-ac', '1', '-i', raw_path,
            '-c:a', 'libopus', '-b:a', '24k', '-frame_duration', '5',
            '-vbr', 'off',
            ogg_path
        ], check=True, capture_output=True)
    except subprocess.CalledProcessError as e:
        sys.exit(f"ffmpeg encode failed: {e.stderr.decode()}")
    finally:
        os.unlink(raw_path)

    # Parse OGG container to extract raw Opus frames.
    frames = parse_ogg_opus(ogg_path)
    os.unlink(ogg_path)
    return frames


def parse_ogg_opus(path: str) -> list:
    """Extract raw Opus frame payloads from an OGG/Opus file."""
    frames = []
    with open(path, 'rb') as f:
        data = f.read()

    pos = 0
    while pos < len(data) - 4:
        # OGG capture pattern: "OggS"
        if data[pos:pos+4] != b'OggS':
            pos += 1
            continue

        if pos + 27 > len(data):
            break

        # header_type: 0=continuation, 2=first, 4=last
        header_type = data[pos + 5]
        # number of segments
        num_segs = data[pos + 26]

        if pos + 27 + num_segs > len(data):
            break

        seg_table = data[pos + 27: pos + 27 + num_segs]
        seg_data_start = pos + 27 + num_segs

        # Each segment is a chunk; a lace ends when a segment < 255.
        offset = seg_data_start
        lace = bytearray()
        for seg_size in seg_table:
            end = offset + seg_size
            if end > len(data):
                break
            lace += data[offset:end]
            offset = end
            if seg_size < 255:
                # End of lace (logical packet)
                if len(lace) > 0:
                    # Skip OpusHead and OpusTags packets (magic bytes)
                    if not (lace[:8] == b'OpusHead' or lace[:8] == b'OpusTags'):
                        frames.append(bytes(lace))
                lace = bytearray()

        # Advance past this OGG page
        pos = offset

    return frames


# ── encoder selection ─────────────────────────────────────────────────────────

def encode_all_frames(pcm_bytes: bytes) -> list:
    """Return list of raw Opus payloads for every 5ms frame of pcm_bytes."""
    frame_bytes = FRAME_SAMPLES * 2  # 16-bit mono

    # Pad to a whole number of frames
    remainder = len(pcm_bytes) % frame_bytes
    if remainder:
        pcm_bytes += b'\x00' * (frame_bytes - remainder)

    num_frames = len(pcm_bytes) // frame_bytes

    # Try opuslib first (frame-by-frame, low latency path)
    test_frame = pcm_bytes[:frame_bytes]
    opus_test = try_opuslib_encode(test_frame)

    if opus_test is not None:
        print("encoder: opuslib", file=sys.stderr)
        frames = [opus_test]
        for i in range(1, num_frames):
            chunk = pcm_bytes[i * frame_bytes: (i + 1) * frame_bytes]
            frames.append(try_opuslib_encode(chunk))
        return frames

    # Fallback: ffmpeg
    print("encoder: ffmpeg (opuslib not available)", file=sys.stderr)
    return encode_all_frames_ffmpeg(pcm_bytes)


# ── main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="anyMic Opus stream sender")
    parser.add_argument('--host',     default='127.0.0.1')
    parser.add_argument('--port',     type=int, default=50127)
    parser.add_argument('--duration', type=float, default=5.0,
                        help='stream duration in seconds')
    parser.add_argument('--signal',   choices=['sine', 'chirp', 'file'],
                        default='sine')
    parser.add_argument('--input',    default=None,
                        help='path to WAV file (required for --signal file)')
    args = parser.parse_args()

    total_samples = int(args.duration * SAMPLE_RATE)

    print(f"Generating {args.signal} PCM ({args.duration:.1f}s = {total_samples} samples)…",
          file=sys.stderr)

    if args.signal == 'sine':
        pcm = gen_sine(total_samples)
    elif args.signal == 'chirp':
        pcm = gen_chirp(total_samples)
    else:
        if not args.input:
            sys.exit("--input required for --signal file")
        pcm = load_wav_pcm(args.input)
        # Truncate / repeat to duration
        target_bytes = total_samples * 2
        while len(pcm) < target_bytes:
            pcm += pcm
        pcm = pcm[:target_bytes]

    print("Encoding to Opus…", file=sys.stderr)
    opus_frames = encode_all_frames(pcm)
    print(f"Encoded {len(opus_frames)} Opus frames", file=sys.stderr)

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    addr = (args.host, args.port)

    seq = 0
    timestamp = 0
    frame_dur = FRAME_SAMPLES / SAMPLE_RATE  # 0.005 s

    print(f"Sending to {args.host}:{args.port} …", file=sys.stderr)

    start = time.monotonic()
    for i, frame in enumerate(opus_frames):
        packet = build_packet(seq, timestamp, frame)
        sock.sendto(packet, addr)

        seq       = (seq + 1) & 0xFFFF
        timestamp = (timestamp + FRAME_SAMPLES) & 0xFFFFFFFF

        # Pace to 5ms intervals
        target_time = start + (i + 1) * frame_dur
        sleep_for = target_time - time.monotonic()
        if sleep_for > 0:
            time.sleep(sleep_for)

    elapsed = time.monotonic() - start
    print(f"Done. Sent {len(opus_frames)} packets in {elapsed:.2f}s "
          f"({len(opus_frames) * frame_dur:.2f}s of audio)",
          file=sys.stderr)

    sock.close()


if __name__ == '__main__':
    main()
