//! anyMic v1 wire packet codec.
//!
//! # Wire format (12-byte header, big-endian)
//!
//! ```text
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | magic  |version | flags  |  pt    |    seq (u16 BE) |
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! |           timestamp (u32 BE)      |   ssrc16 (u16 BE)       |
//! +--------+--------+--------+--------+--------+--------+
//! |  ... payload bytes ...
//! ```
//!
//! # Parsing strictness
//!
//! This implementation uses **strict mode** for the reserved flag bits (bits 2–7).
//! Any packet with those bits non-zero is rejected with [`PacketError::ReservedFlagsSet`].
//! Rationale: enforcing zero reserved bits now prevents silent interoperability
//! problems if a future version assigns meaning to those bits.

use std::borrow::Cow;
use std::fmt;

// ───────────────────────────── constants ─────────────────────────────────────

pub const MAGIC: u8 = 0xA1;
pub const VERSION_MAJOR: u8 = 1;
pub const VERSION_MINOR: u8 = 0;
pub const HEADER_LEN: usize = 12;
pub const MAX_PACKET_LEN: usize = 1500;

/// Mask selecting bits 2-7 of the flags byte (reserved, must be zero).
const FLAGS_RESERVED_MASK: u8 = 0xFC;

// ───────────────────────────── PayloadType ───────────────────────────────────

/// Known payload types carried in the anyMic header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadType {
    /// Opus codec, 48 kHz, mono.
    Opus48kMono = 1,
    /// Raw 16-bit PCM (debugging / development only).
    Pcm16Raw = 2,
}

impl PayloadType {
    fn from_u8(v: u8) -> Result<Self, PacketError> {
        match v {
            1 => Ok(PayloadType::Opus48kMono),
            2 => Ok(PayloadType::Pcm16Raw),
            other => Err(PacketError::UnknownPayloadType(other)),
        }
    }
}

impl fmt::Display for PayloadType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadType::Opus48kMono => f.write_str("Opus48kMono"),
            PayloadType::Pcm16Raw => f.write_str("Pcm16Raw"),
        }
    }
}

// ───────────────────────────── Flags ─────────────────────────────────────────

/// Bit-field flags from byte 2 of the header.
///
/// `marker` (bit 0): set on the first packet of a speech burst (talk-spurt start).
/// `dtx`    (bit 1): discontinuous-transmission — sender is suppressing silence.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    pub marker: bool,
    pub dtx: bool,
}

impl Flags {
    /// Encode to the wire byte (reserved bits always 0).
    #[inline]
    pub fn to_u8(self) -> u8 {
        (self.marker as u8) | ((self.dtx as u8) << 1)
    }

    /// Decode from the wire byte.  Returns `Err` if reserved bits are set.
    #[inline]
    pub fn from_u8(v: u8) -> Result<Self, PacketError> {
        if v & FLAGS_RESERVED_MASK != 0 {
            return Err(PacketError::ReservedFlagsSet(v));
        }
        Ok(Flags {
            marker: v & 0x01 != 0,
            dtx: v & 0x02 != 0,
        })
    }
}

// ───────────────────────────── RtpPacket ─────────────────────────────────────

/// An anyMic packet with its header fields and payload.
///
/// The lifetime `'a` lets [`RtpPacket::decode`] borrow the payload slice directly
/// from the input buffer (zero-copy), while [`RtpPacket::decode_owned`] — or any
/// packet constructed by the caller — stores an owned `Vec<u8>` via `Cow::Owned`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket<'a> {
    /// Minor version from the low 4 bits of byte 1.
    pub version_minor: u8,
    /// Parsed flag bits.
    pub flags: Flags,
    /// Payload type identifier.
    pub payload_type: PayloadType,
    /// Sequence number (wraps at 2^16).
    pub seq: u16,
    /// Timestamp in 48 kHz samples (wraps at 2^32).
    pub timestamp: u32,
    /// 16-bit synchronisation source identifier.
    pub ssrc16: u16,
    /// Raw codec bytes following the 12-byte header.
    pub payload: Cow<'a, [u8]>,
}

impl<'a> RtpPacket<'a> {
    // ── decode ────────────────────────────────────────────────────────────────

    /// Decode a packet, borrowing the payload slice from `bytes` (zero-copy).
    ///
    /// Returns an error for any of the protocol violations documented on
    /// [`PacketError`].
    pub fn decode(bytes: &'a [u8]) -> Result<RtpPacket<'a>, PacketError> {
        let total = bytes.len();
        if total < HEADER_LEN {
            return Err(PacketError::Truncated(total));
        }
        if total > MAX_PACKET_LEN {
            return Err(PacketError::TooLarge(total));
        }

        // byte 0 – magic
        if bytes[0] != MAGIC {
            return Err(PacketError::InvalidMagic(bytes[0]));
        }

        // byte 1 – version (major in high nibble, minor in low nibble)
        let major = bytes[1] >> 4;
        let minor = bytes[1] & 0x0F;
        if major != VERSION_MAJOR {
            return Err(PacketError::UnsupportedMajor(major));
        }

        // byte 2 – flags (strict: reserved bits must be 0)
        let flags = Flags::from_u8(bytes[2])?;

        // byte 3 – payload type
        let payload_type = PayloadType::from_u8(bytes[3])?;

        // bytes 4-5 – seq (u16 BE)
        let seq = u16::from_be_bytes([bytes[4], bytes[5]]);

        // bytes 6-9 – timestamp (u32 BE)
        let timestamp = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);

        // bytes 10-11 – ssrc16 (u16 BE)
        let ssrc16 = u16::from_be_bytes([bytes[10], bytes[11]]);

        Ok(RtpPacket {
            version_minor: minor,
            flags,
            payload_type,
            seq,
            timestamp,
            ssrc16,
            payload: Cow::Borrowed(&bytes[HEADER_LEN..]),
        })
    }

    /// Decode a packet, cloning the payload into an owned buffer.
    pub fn decode_owned(bytes: &[u8]) -> Result<RtpPacket<'static>, PacketError> {
        // Parse header fields using the same validation logic as decode(), but
        // the resulting packet carries Cow::Owned so no lifetime ties to `bytes`.
        let total = bytes.len();
        if total < HEADER_LEN {
            return Err(PacketError::Truncated(total));
        }
        if total > MAX_PACKET_LEN {
            return Err(PacketError::TooLarge(total));
        }
        if bytes[0] != MAGIC {
            return Err(PacketError::InvalidMagic(bytes[0]));
        }
        let major = bytes[1] >> 4;
        let minor = bytes[1] & 0x0F;
        if major != VERSION_MAJOR {
            return Err(PacketError::UnsupportedMajor(major));
        }
        let flags = Flags::from_u8(bytes[2])?;
        let payload_type = PayloadType::from_u8(bytes[3])?;
        let seq = u16::from_be_bytes([bytes[4], bytes[5]]);
        let timestamp = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
        let ssrc16 = u16::from_be_bytes([bytes[10], bytes[11]]);
        Ok(RtpPacket {
            version_minor: minor,
            flags,
            payload_type,
            seq,
            timestamp,
            ssrc16,
            payload: Cow::Owned(bytes[HEADER_LEN..].to_vec()),
        })
    }

    // ── encode ────────────────────────────────────────────────────────────────

    /// Number of bytes this packet will occupy on the wire.
    #[inline]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    /// Encode the header and payload into `out`, appending to whatever is
    /// already there.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.reserve(self.encoded_len());

        // byte 0
        out.push(MAGIC);
        // byte 1 – major in high nibble, minor in low
        out.push((VERSION_MAJOR << 4) | (self.version_minor & 0x0F));
        // byte 2 – flags
        out.push(self.flags.to_u8());
        // byte 3 – payload type
        out.push(self.payload_type as u8);
        // bytes 4-5 – seq BE
        out.extend_from_slice(&self.seq.to_be_bytes());
        // bytes 6-9 – timestamp BE
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        // bytes 10-11 – ssrc16 BE
        out.extend_from_slice(&self.ssrc16.to_be_bytes());
        // payload
        out.extend_from_slice(&self.payload);
    }
}

impl fmt::Display for RtpPacket<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "v1 seq={} ts={} ssrc={} pt={} flags={}{}len={}",
            self.seq,
            self.timestamp,
            self.ssrc16,
            self.payload_type,
            if self.flags.marker { "M" } else { "" },
            if self.flags.dtx { "D " } else { " " },
            self.payload.len(),
        )
    }
}

// ───────────────────────────── errors ────────────────────────────────────────

/// Errors returned by the packet codec.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PacketError {
    /// Buffer shorter than the 12-byte header.
    #[error("buffer too short: {0} bytes")]
    Truncated(usize),
    /// Byte 0 was not `0xA1`.
    #[error("invalid magic: 0x{0:02X}")]
    InvalidMagic(u8),
    /// The major version in the high nibble of byte 1 is not 1.
    #[error("unsupported major version: {0}")]
    UnsupportedMajor(u8),
    /// Reserved flag bits (2–7) are non-zero (strict mode).
    #[error("reserved flag bits set: 0x{0:02X}")]
    ReservedFlagsSet(u8),
    /// The payload-type byte is not in the known set.
    #[error("unknown payload type: {0}")]
    UnknownPayloadType(u8),
    /// Total packet length exceeds [`MAX_PACKET_LEN`].
    #[error("packet too large: {0} bytes (max {})", MAX_PACKET_LEN)]
    TooLarge(usize),
}

// ───────────────────────────── seq/ts arithmetic ──────────────────────────────

/// Signed difference between two 16-bit sequence numbers, handling wrap-around.
///
/// Based on RFC 1982 serial number arithmetic.  Returns a value in `[-32768, 32767]`.
///
/// ```
/// use anymic_core::packet::seq_diff;
/// assert_eq!(seq_diff(0, 65535), 1);
/// assert_eq!(seq_diff(65535, 0), -1);
/// assert_eq!(seq_diff(100, 100), 0);
/// ```
#[inline]
pub fn seq_diff(a: u16, b: u16) -> i32 {
    // Compute the raw unsigned difference in the u16 space, then reinterpret
    // as signed using the midpoint of the u16 range as the sign boundary.
    let diff = a.wrapping_sub(b) as i16;
    diff as i32
}

/// Signed difference between two 32-bit timestamps, handling wrap-around.
///
/// Returns a value in `[-2^31, 2^31-1]`.
///
/// ```
/// use anymic_core::packet::ts_diff;
/// assert_eq!(ts_diff(0, u32::MAX), 1);
/// assert_eq!(ts_diff(u32::MAX, 0), -1);
/// assert_eq!(ts_diff(500, 500), 0);
/// ```
#[inline]
pub fn ts_diff(a: u32, b: u32) -> i64 {
    let diff = a.wrapping_sub(b) as i32;
    diff as i64
}

// ───────────────────────────── unit tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_packet() -> RtpPacket<'static> {
        RtpPacket {
            version_minor: 0,
            flags: Flags {
                marker: false,
                dtx: false,
            },
            payload_type: PayloadType::Opus48kMono,
            seq: 1,
            timestamp: 960,
            ssrc16: 0xBEEF,
            payload: Cow::Owned(vec![0xDE, 0xAD]),
        }
    }

    // ── golden encode ─────────────────────────────────────────────────────────

    #[test]
    fn golden_encode() {
        let pkt = base_packet();
        let mut buf = Vec::new();
        pkt.encode_into(&mut buf);
        // Expected: A1 10 00 01 00 01 00 00 03 C0 BE EF DE AD
        assert_eq!(
            buf,
            &[
                0xA1, 0x10, 0x00, 0x01, // magic, version, flags, pt
                0x00, 0x01, // seq=1
                0x00, 0x00, 0x03, 0xC0, // timestamp=960
                0xBE, 0xEF, // ssrc16=0xBEEF
                0xDE, 0xAD, // payload
            ]
        );
    }

    // ── encoded_len ───────────────────────────────────────────────────────────

    #[test]
    fn encoded_len_matches_buffer() {
        let pkt = base_packet();
        let mut buf = Vec::new();
        pkt.encode_into(&mut buf);
        assert_eq!(pkt.encoded_len(), buf.len());
    }

    // ── round-trip (borrowed) ─────────────────────────────────────────────────

    #[test]
    fn round_trip_borrowed() {
        let orig = base_packet();
        let mut buf = Vec::new();
        orig.encode_into(&mut buf);
        let decoded = RtpPacket::decode(&buf).unwrap();
        assert_eq!(decoded.version_minor, orig.version_minor);
        assert_eq!(decoded.flags, orig.flags);
        assert_eq!(decoded.payload_type, orig.payload_type);
        assert_eq!(decoded.seq, orig.seq);
        assert_eq!(decoded.timestamp, orig.timestamp);
        assert_eq!(decoded.ssrc16, orig.ssrc16);
        assert_eq!(&*decoded.payload, &*orig.payload);
    }

    // ── round-trip (owned) ────────────────────────────────────────────────────

    #[test]
    fn round_trip_owned() {
        let orig = base_packet();
        let mut buf = Vec::new();
        orig.encode_into(&mut buf);
        let decoded = RtpPacket::decode_owned(&buf).unwrap();
        assert_eq!(decoded.seq, orig.seq);
        assert_eq!(&*decoded.payload, &*orig.payload);
    }

    // ── truncated ─────────────────────────────────────────────────────────────

    #[test]
    fn truncated_all_short_lengths() {
        for len in 0..HEADER_LEN {
            let bytes = vec![0u8; len];
            assert_eq!(
                RtpPacket::decode(&bytes),
                Err(PacketError::Truncated(len)),
                "expected Truncated for len={len}"
            );
        }
    }

    // ── bad magic ─────────────────────────────────────────────────────────────

    #[test]
    fn bad_magic() {
        let mut buf = Vec::new();
        base_packet().encode_into(&mut buf);
        buf[0] = 0xFF;
        assert_eq!(
            RtpPacket::decode(&buf),
            Err(PacketError::InvalidMagic(0xFF))
        );
    }

    // ── bad major version ─────────────────────────────────────────────────────

    #[test]
    fn bad_major_version() {
        let mut buf = Vec::new();
        base_packet().encode_into(&mut buf);
        buf[1] = 0x21; // major=2, minor=1
        assert_eq!(
            RtpPacket::decode(&buf),
            Err(PacketError::UnsupportedMajor(2))
        );
    }

    // ── reserved flags set ────────────────────────────────────────────────────

    #[test]
    fn reserved_flags_set() {
        let mut buf = Vec::new();
        base_packet().encode_into(&mut buf);
        buf[2] = 0xFC; // all reserved bits set, marker+dtx clear
        assert_eq!(
            RtpPacket::decode(&buf),
            Err(PacketError::ReservedFlagsSet(0xFC))
        );
    }

    // ── unknown payload type ──────────────────────────────────────────────────

    #[test]
    fn unknown_payload_type() {
        let mut buf = Vec::new();
        base_packet().encode_into(&mut buf);
        buf[3] = 0xFF;
        assert_eq!(
            RtpPacket::decode(&buf),
            Err(PacketError::UnknownPayloadType(0xFF))
        );
    }

    // ── too large ─────────────────────────────────────────────────────────────

    #[test]
    fn too_large() {
        let big = vec![0u8; MAX_PACKET_LEN + 1];
        assert_eq!(
            RtpPacket::decode(&big),
            Err(PacketError::TooLarge(MAX_PACKET_LEN + 1))
        );
    }

    // ── exact max size is OK ──────────────────────────────────────────────────

    #[test]
    fn exact_max_size_ok() {
        let mut buf = Vec::new();
        base_packet().encode_into(&mut buf);
        // Pad to exactly MAX_PACKET_LEN
        buf.resize(MAX_PACKET_LEN, 0);
        assert!(RtpPacket::decode(&buf).is_ok());
    }

    // ── flags encoding ────────────────────────────────────────────────────────

    #[test]
    fn flags_all_combinations() {
        for (marker, dtx, expected) in [
            (false, false, 0u8),
            (true, false, 1),
            (false, true, 2),
            (true, true, 3),
        ] {
            let f = Flags { marker, dtx };
            assert_eq!(f.to_u8(), expected);
            let f2 = Flags::from_u8(expected).unwrap();
            assert_eq!(f2.marker, marker);
            assert_eq!(f2.dtx, dtx);
        }
    }

    // ── seq_diff wrap-aware ───────────────────────────────────────────────────

    #[test]
    fn seq_diff_wrap_aware() {
        assert_eq!(seq_diff(0, 65535), 1, "0 - 65535 should be +1 (wrap)");
        assert_eq!(seq_diff(65535, 0), -1, "65535 - 0 should be -1 (wrap)");
        assert_eq!(seq_diff(100, 100), 0, "equal seqs should diff to 0");
        assert_eq!(seq_diff(200, 100), 100);
        assert_eq!(seq_diff(100, 200), -100);
        // Half-range edge cases
        assert_eq!(seq_diff(32768, 0), -32768); // exact midpoint, negative
        assert_eq!(seq_diff(32767, 0), 32767);
    }

    // ── ts_diff wrap-aware ────────────────────────────────────────────────────

    #[test]
    fn ts_diff_wrap_aware() {
        assert_eq!(ts_diff(0, u32::MAX), 1);
        assert_eq!(ts_diff(u32::MAX, 0), -1);
        assert_eq!(ts_diff(500, 500), 0);
        assert_eq!(ts_diff(1000, 500), 500);
        assert_eq!(ts_diff(500, 1000), -500);
    }

    // ── Display ───────────────────────────────────────────────────────────────

    #[test]
    fn display_contains_key_fields() {
        let pkt = base_packet();
        let s = pkt.to_string();
        assert!(s.contains("v1"), "display should contain 'v1': {s}");
        assert!(s.contains("seq=1"), "display should contain seq: {s}");
        assert!(s.contains("ts=960"), "display should contain ts: {s}");
        assert!(s.contains("ssrc="), "display should contain ssrc: {s}");
        assert!(s.contains("Opus48kMono"), "display should contain pt: {s}");
        assert!(s.contains("len=2"), "display should contain len: {s}");
    }

    // ── empty payload ─────────────────────────────────────────────────────────

    #[test]
    fn empty_payload_round_trip() {
        let mut pkt = base_packet();
        pkt.payload = Cow::Owned(vec![]);
        let mut buf = Vec::new();
        pkt.encode_into(&mut buf);
        assert_eq!(buf.len(), HEADER_LEN);
        let decoded = RtpPacket::decode(&buf).unwrap();
        assert_eq!(decoded.payload.len(), 0);
    }

    // ── PCM16 payload type ────────────────────────────────────────────────────

    #[test]
    fn pcm16_payload_type_round_trip() {
        let mut pkt = base_packet();
        pkt.payload_type = PayloadType::Pcm16Raw;
        let mut buf = Vec::new();
        pkt.encode_into(&mut buf);
        let decoded = RtpPacket::decode(&buf).unwrap();
        assert_eq!(decoded.payload_type, PayloadType::Pcm16Raw);
    }
}
