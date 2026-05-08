//! Integration tests for the anyMic v1 packet codec.
//!
//! Tests cover:
//! 1. Golden encoding (known inputs → expected hex bytes)
//! 2. Round-trip (encode → decode → field equality)
//! 3. Truncated input (all lengths 0..12 must return Truncated)
//! 4. Bad magic (0xFF → InvalidMagic)
//! 5. Bad major version (major=2 → UnsupportedMajor)
//! 6. Reserved flags set (0xFC → ReservedFlagsSet)
//! 7. Wrap-aware seq_diff arithmetic
//! 8. proptest round-trip (≥ 1 000 cases, ≥ 5 s timeout)

use anymic_core::packet::{
    seq_diff, ts_diff, Flags, PacketError, PayloadType, RtpPacket, HEADER_LEN, MAX_PACKET_LEN,
};
use std::borrow::Cow;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_packet<'a>(
    version_minor: u8,
    marker: bool,
    dtx: bool,
    pt: PayloadType,
    seq: u16,
    ts: u32,
    ssrc: u16,
    payload: &'a [u8],
) -> RtpPacket<'a> {
    RtpPacket {
        version_minor,
        flags: Flags { marker, dtx },
        payload_type: pt,
        seq,
        timestamp: ts,
        ssrc16: ssrc,
        payload: Cow::Borrowed(payload),
    }
}

// ── 1. Golden encoding ────────────────────────────────────────────────────────

#[test]
fn golden_encode_opus() {
    // seq=0x0100 (256), ts=0x000186A0 (100000), ssrc=0xCAFE, payload=[0x01,0x02,0x03]
    let payload = [0x01u8, 0x02, 0x03];
    let pkt = make_packet(
        0,
        true,
        false,
        PayloadType::Opus48kMono,
        0x0100,
        0x0001_86A0,
        0xCAFE,
        &payload,
    );
    let mut buf = Vec::new();
    pkt.encode_into(&mut buf);

    #[rustfmt::skip]
    let expected: &[u8] = &[
        0xA1,       // magic
        0x10,       // version: major=1, minor=0
        0x01,       // flags: marker=1, dtx=0
        0x01,       // payload_type: Opus48kMono
        0x01, 0x00, // seq = 256
        0x00, 0x01, 0x86, 0xA0, // timestamp = 100000
        0xCA, 0xFE, // ssrc16
        0x01, 0x02, 0x03, // payload
    ];
    assert_eq!(buf, expected, "golden encode mismatch");
}

#[test]
fn golden_encode_pcm16_dtx() {
    let payload = [0xAA, 0xBBu8];
    let pkt = make_packet(
        3,
        false,
        true,
        PayloadType::Pcm16Raw,
        0xFFFF,
        0xFFFF_FFFF,
        0x0001,
        &payload,
    );
    let mut buf = Vec::new();
    pkt.encode_into(&mut buf);

    #[rustfmt::skip]
    let expected: &[u8] = &[
        0xA1,       // magic
        0x13,       // version: major=1, minor=3
        0x02,       // flags: marker=0, dtx=1
        0x02,       // payload_type: Pcm16Raw
        0xFF, 0xFF, // seq = 65535
        0xFF, 0xFF, 0xFF, 0xFF, // timestamp = u32::MAX
        0x00, 0x01, // ssrc16 = 1
        0xAA, 0xBB, // payload
    ];
    assert_eq!(buf, expected, "golden PCM16+DTX encode mismatch");
}

// ── 2. Round-trip ─────────────────────────────────────────────────────────────

#[test]
fn round_trip_borrowed() {
    let payload = b"hello anyMic";
    let orig = make_packet(
        0,
        true,
        true,
        PayloadType::Opus48kMono,
        42,
        96000,
        0x1234,
        payload,
    );
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    let decoded = RtpPacket::decode(&buf).expect("decode failed");

    assert_eq!(decoded.version_minor, orig.version_minor);
    assert_eq!(decoded.flags.marker, orig.flags.marker);
    assert_eq!(decoded.flags.dtx, orig.flags.dtx);
    assert_eq!(decoded.payload_type, orig.payload_type);
    assert_eq!(decoded.seq, orig.seq);
    assert_eq!(decoded.timestamp, orig.timestamp);
    assert_eq!(decoded.ssrc16, orig.ssrc16);
    assert_eq!(&*decoded.payload, &*orig.payload);
}

#[test]
fn round_trip_owned() {
    let payload = b"owned round trip";
    let orig = make_packet(
        0,
        false,
        false,
        PayloadType::Pcm16Raw,
        999,
        48000,
        7,
        payload,
    );
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    let decoded = RtpPacket::decode_owned(&buf).expect("decode_owned failed");

    assert_eq!(decoded.seq, orig.seq);
    assert_eq!(decoded.timestamp, orig.timestamp);
    assert_eq!(decoded.ssrc16, orig.ssrc16);
    assert_eq!(&*decoded.payload, payload.as_slice());
}

#[test]
fn round_trip_empty_payload() {
    let orig = make_packet(0, false, false, PayloadType::Opus48kMono, 0, 0, 0, &[]);
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    assert_eq!(buf.len(), HEADER_LEN);
    let decoded = RtpPacket::decode(&buf).unwrap();
    assert_eq!(decoded.payload.len(), 0);
}

// ── 3. Truncated ─────────────────────────────────────────────────────────────

#[test]
fn truncated_every_short_length() {
    for len in 0..HEADER_LEN {
        let bytes = vec![0u8; len];
        assert_eq!(
            RtpPacket::decode(&bytes),
            Err(PacketError::Truncated(len)),
            "expected Truncated for len={len}"
        );
    }
}

// ── 4. Bad magic ─────────────────────────────────────────────────────────────

#[test]
fn bad_magic_0xff() {
    let mut buf = vec![0u8; HEADER_LEN];
    buf[0] = 0xFF;
    assert_eq!(
        RtpPacket::decode(&buf),
        Err(PacketError::InvalidMagic(0xFF))
    );
}

#[test]
fn bad_magic_zero() {
    let buf = vec![0u8; HEADER_LEN];
    assert_eq!(
        RtpPacket::decode(&buf),
        Err(PacketError::InvalidMagic(0x00))
    );
}

// ── 5. Bad major version ──────────────────────────────────────────────────────

#[test]
fn bad_major_version_2() {
    // Byte 1 = 0x21 → major = 2, minor = 1
    let payload = b"test";
    let orig = make_packet(
        0,
        false,
        false,
        PayloadType::Opus48kMono,
        1,
        960,
        1,
        payload,
    );
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    buf[1] = 0x21; // major=2
    assert_eq!(
        RtpPacket::decode(&buf),
        Err(PacketError::UnsupportedMajor(2))
    );
}

#[test]
fn bad_major_version_0() {
    let orig = make_packet(0, false, false, PayloadType::Opus48kMono, 1, 960, 1, b"");
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    buf[1] = 0x00; // major=0
    assert_eq!(
        RtpPacket::decode(&buf),
        Err(PacketError::UnsupportedMajor(0))
    );
}

// ── 6. Reserved flags set ─────────────────────────────────────────────────────

#[test]
fn reserved_flags_0xfc() {
    let orig = make_packet(0, false, false, PayloadType::Opus48kMono, 1, 960, 1, b"");
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    buf[2] = 0xFC;
    assert_eq!(
        RtpPacket::decode(&buf),
        Err(PacketError::ReservedFlagsSet(0xFC))
    );
}

#[test]
fn reserved_flags_single_bit() {
    let orig = make_packet(0, false, false, PayloadType::Opus48kMono, 1, 960, 1, b"");
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    buf[2] = 0x04; // only bit 2 set (first reserved bit)
    assert_eq!(
        RtpPacket::decode(&buf),
        Err(PacketError::ReservedFlagsSet(0x04))
    );
}

#[test]
fn valid_flags_both_low_bits() {
    let orig = make_packet(0, true, true, PayloadType::Opus48kMono, 1, 960, 1, b"");
    let mut buf = Vec::new();
    orig.encode_into(&mut buf);
    // flags byte = 0x03 (marker=1, dtx=1), no reserved bits
    let decoded = RtpPacket::decode(&buf).expect("should be valid");
    assert!(decoded.flags.marker);
    assert!(decoded.flags.dtx);
}

// ── 7. Wrap-aware seq_diff ────────────────────────────────────────────────────

#[test]
fn seq_diff_wrap_cases() {
    assert_eq!(seq_diff(0, 65535), 1, "0 - 65535 wraps to +1");
    assert_eq!(seq_diff(65535, 0), -1, "65535 - 0 wraps to -1");
    assert_eq!(seq_diff(100, 100), 0, "equal seqs → 0");
    assert_eq!(seq_diff(200, 100), 100);
    assert_eq!(seq_diff(100, 200), -100);
    // Stress the boundary at the half-range
    assert_eq!(seq_diff(32767, 0), 32767);
}

#[test]
fn ts_diff_wrap_cases() {
    assert_eq!(ts_diff(0, u32::MAX), 1);
    assert_eq!(ts_diff(u32::MAX, 0), -1);
    assert_eq!(ts_diff(500, 500), 0);
    assert_eq!(ts_diff(96000, 48000), 48000);
    assert_eq!(ts_diff(48000, 96000), -48000);
}

// ── 8. proptest round-trip ────────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    // Run at least 1 000 cases; set a generous timeout via env var if needed.
    #![proptest_config(ProptestConfig {
        cases: 1024,
        // Allow up to 10 s (10_000 ms) per test run to satisfy the ≥5 s requirement
        // even on slow CI machines.
        timeout: 10_000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn proptest_round_trip(
        version_minor in 0u8..=15u8,
        marker in any::<bool>(),
        dtx in any::<bool>(),
        // Only valid payload types: 1 or 2
        pt_raw in 1u8..=2u8,
        seq in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc16 in any::<u16>(),
        // Keep payload small enough to stay under MAX_PACKET_LEN (12 + payload ≤ 1500)
        payload in proptest::collection::vec(any::<u8>(), 0..(MAX_PACKET_LEN - HEADER_LEN)),
    ) {
        let pt = if pt_raw == 1 { PayloadType::Opus48kMono } else { PayloadType::Pcm16Raw };
        let orig = RtpPacket {
            version_minor,
            flags: Flags { marker, dtx },
            payload_type: pt,
            seq,
            timestamp,
            ssrc16,
            payload: Cow::Owned(payload.clone()),
        };

        let mut buf = Vec::new();
        orig.encode_into(&mut buf);

        let decoded = RtpPacket::decode(&buf)
            .expect("round-trip decode must not fail for valid inputs");

        prop_assert_eq!(decoded.version_minor, orig.version_minor);
        prop_assert_eq!(decoded.flags.marker, orig.flags.marker);
        prop_assert_eq!(decoded.flags.dtx, orig.flags.dtx);
        prop_assert_eq!(decoded.payload_type, orig.payload_type);
        prop_assert_eq!(decoded.seq, orig.seq);
        prop_assert_eq!(decoded.timestamp, orig.timestamp);
        prop_assert_eq!(decoded.ssrc16, orig.ssrc16);
        prop_assert_eq!(&*decoded.payload, payload.as_slice());
    }

    #[test]
    fn proptest_seq_diff_inverse(a in any::<u16>(), b in any::<u16>()) {
        // seq_diff(a, b) == -seq_diff(b, a) unless at the exact midpoint
        let d1 = seq_diff(a, b);
        let d2 = seq_diff(b, a);
        if d1 != i32::from(i16::MIN) {
            prop_assert_eq!(d1, -d2);
        }
    }

    #[test]
    fn proptest_ts_diff_self_zero(v in any::<u32>()) {
        prop_assert_eq!(ts_diff(v, v), 0);
    }
}
