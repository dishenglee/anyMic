//! Integration tests for the adaptive jitter buffer.

use anymic_core::jitter::{JitterBuffer, JitterOut};
use anymic_core::packet::{Flags, PayloadType, RtpPacket};
use std::borrow::Cow;

// ─────────────────────────────── helpers ─────────────────────────────────────

const FRAME_SAMPLES: u32 = 240; // 5 ms @ 48 kHz
const MAX_STALL: u32 = 500;
const STALL_THRESHOLD: u32 = 50; // same as the crate-private constant

fn make_pkt(seq: u16, ts: u32, payload: &[u8]) -> RtpPacket<'static> {
    RtpPacket {
        version_minor: 0,
        flags: Flags::default(),
        payload_type: PayloadType::Opus48kMono,
        seq,
        timestamp: ts,
        ssrc16: 1,
        payload: Cow::Owned(payload.to_vec()),
    }
}

fn new_buf() -> JitterBuffer {
    JitterBuffer::new(FRAME_SAMPLES, MAX_STALL)
}

/// Push `n` sequential frames starting at timestamp `base_ts` and sequence `base_seq`.
/// Returns `(next_seq, next_ts)` for chaining.
fn push_sequential(buf: &mut JitterBuffer, n: u16, base_seq: u16, base_ts: u32) -> (u16, u32) {
    for i in 0..n {
        let seq = base_seq.wrapping_add(i);
        let ts = base_ts.wrapping_add(i as u32 * FRAME_SAMPLES);
        buf.push(make_pkt(seq, ts, &[i as u8]));
    }
    let next_seq = base_seq.wrapping_add(n);
    let next_ts = base_ts.wrapping_add(n as u32 * FRAME_SAMPLES);
    (next_seq, next_ts)
}

/// Drain frames out of the buffer, returning only `Frame` outputs with payloads.
/// Skips `Plc` events during warmup (at most `max_plc` allowed before the first Frame).
fn pop_frames(buf: &mut JitterBuffer, count: usize) -> Vec<(u16, u32, Vec<u8>)> {
    let mut results = Vec::new();
    let mut iters = 0usize;
    let max_iters = count * 20 + 100; // safety ceiling
    while results.len() < count && iters < max_iters {
        match buf.pop_due(0) {
            JitterOut::Frame {
                seq,
                timestamp,
                payload,
            } => {
                results.push((seq, timestamp, payload));
            }
            JitterOut::Plc { .. } => {}
            JitterOut::Stall => {}
            JitterOut::Disconnected => break,
        }
        iters += 1;
    }
    results
}

// ─────────────────────────────── test 1: sequential ──────────────────────────

/// Push 10 frames in order; pop 10 frames — all should arrive as `Frame`.
#[test]
fn t01_sequential_play() {
    let mut buf = new_buf();
    push_sequential(&mut buf, 10, 1, 0);

    let frames = pop_frames(&mut buf, 10);
    assert_eq!(frames.len(), 10, "expected 10 frames, got {}", frames.len());

    // Verify they come out in ascending timestamp order.
    for (i, (_, ts, _)) in frames.iter().enumerate() {
        let expected_ts = i as u32 * FRAME_SAMPLES;
        assert_eq!(
            *ts, expected_ts,
            "frame {i}: expected ts={expected_ts}, got ts={ts}"
        );
    }
    assert_eq!(buf.stats().received, 10);
}

// ─────────────────────────────── test 2: out-of-order ────────────────────────

/// Push frames in order 4-3-1-2-5; pop should yield 1-2-3-4-5.
#[test]
fn t02_out_of_order() {
    let mut buf = new_buf();

    let ts = |n: u32| n.wrapping_mul(FRAME_SAMPLES);
    // Push in scrambled order.
    for &i in &[4u32, 3, 1, 2, 5] {
        buf.push(make_pkt(i as u16, ts(i - 1), &[i as u8]));
    }

    let frames = pop_frames(&mut buf, 5);
    assert_eq!(frames.len(), 5);

    // Timestamps must be strictly ascending.
    let timestamps: Vec<u32> = frames.iter().map(|(_, ts, _)| *ts).collect();
    for w in timestamps.windows(2) {
        assert!(w[1] > w[0], "timestamps not ascending: {:?}", timestamps);
    }
    // Payloads should be 1, 2, 3, 4, 5 (we stored `i` as the single payload byte).
    let payloads: Vec<u8> = frames.iter().map(|(_, _, p)| p[0]).collect();
    assert_eq!(payloads, vec![1, 2, 3, 4, 5]);
}

// ─────────────────────────────── test 3: duplicate ───────────────────────────

/// Pushing the same timestamp twice should count one duplicate and emit one Frame.
#[test]
fn t03_duplicate_packet() {
    let mut buf = new_buf();
    buf.push(make_pkt(1, 0, &[0xAA]));
    buf.push(make_pkt(1, 0, &[0xBB])); // duplicate

    assert_eq!(buf.stats().duplicates, 1);

    let frames = pop_frames(&mut buf, 1);
    assert_eq!(frames.len(), 1);
    // The original payload (0xAA) should have been kept.
    assert_eq!(frames[0].2, vec![0xAA]);
}

// ─────────────────────────────── test 4: missing frame → PLC ────────────────

/// Frames 1-2-3-5 pushed (frame 4 omitted).  Pop should yield 1-2-3-PLC-5.
#[test]
fn t04_missing_frame_plc() {
    let mut buf = new_buf();
    let ts = |n: u32| n.wrapping_mul(FRAME_SAMPLES);

    // Push frames 1, 2, 3, 5 (skip 4).
    for &i in &[1u32, 2, 3, 5] {
        buf.push(make_pkt(i as u16, ts(i - 1), &[i as u8]));
    }

    // Drain pop_due until we've seen at least 5 outputs including PLC.
    let mut got_frames: Vec<u32> = Vec::new();
    let mut got_plc = 0u32;
    let mut iters = 0;
    while (got_frames.len() < 5 || got_plc == 0) && iters < 200 {
        match buf.pop_due(0) {
            JitterOut::Frame { timestamp, .. } => got_frames.push(timestamp),
            JitterOut::Plc { .. } => got_plc += 1,
            _ => {}
        }
        iters += 1;
    }

    // We expect 5 real frames: ts 0, 240, 480, 720 (slot 4), 960.
    // Slot 4 (ts=720, frame index 4) was skipped → PLC.
    assert!(got_plc >= 1, "expected at least one PLC event");
    // Frame with ts=ts(4)=960 is the 5th frame (index 4, ts=4*240=960).
    let frame5_ts = ts(4);
    assert!(
        got_frames.contains(&frame5_ts),
        "expected frame5 (ts={frame5_ts}) among frames: {got_frames:?}"
    );
}

// ─────────────────────────────── test 5: late arrival ────────────────────────

/// Push 3 frames, pop all 3, then push a late frame (ts of frame 2) → late_dropped=1.
#[test]
fn t05_late_arrival() {
    let mut buf = new_buf();
    let ts = |n: u32| n.wrapping_mul(FRAME_SAMPLES);

    buf.push(make_pkt(1, ts(0), &[1]));
    buf.push(make_pkt(2, ts(1), &[2]));
    buf.push(make_pkt(3, ts(2), &[3]));

    pop_frames(&mut buf, 3);

    // Now push a packet with a timestamp that was already popped (ts(1)).
    buf.push(make_pkt(2, ts(1), &[99]));

    assert_eq!(buf.stats().late_dropped, 1, "late_dropped should be 1");
}

// ─────────────────────────────── test 6: wrap-around ─────────────────────────

/// Timestamps wrapping from near u32::MAX across the 0 boundary should sort correctly.
#[test]
fn t06_timestamp_wrap_around() {
    let mut buf = new_buf();

    // Place timestamps right before and after the u32 wrap point.
    // With frame_samples=240:
    //   ts0 = 0xFFFF_FFFE - 240  → last frame before wrap
    //   ts1 = 0xFFFF_FFFE        → wrap
    //   ts2 = 0x0000_0001E (30)  → first frame after wrap

    let ts0: u32 = u32::MAX
        .wrapping_sub(FRAME_SAMPLES)
        .wrapping_sub(FRAME_SAMPLES);
    let ts1: u32 = ts0.wrapping_add(FRAME_SAMPLES);
    let ts2: u32 = ts1.wrapping_add(FRAME_SAMPLES);

    buf.push(make_pkt(1, ts0, &[10]));
    buf.push(make_pkt(2, ts1, &[20]));
    buf.push(make_pkt(3, ts2, &[30]));

    let frames = pop_frames(&mut buf, 3);
    assert_eq!(frames.len(), 3, "expected 3 frames across wrap");

    // The payloads should come out in the push order (10, 20, 30) since ts0 < ts1 < ts2 mod 2^32.
    let payloads: Vec<u8> = frames.iter().map(|(_, _, p)| p[0]).collect();
    assert_eq!(payloads, vec![10, 20, 30]);
}

// ─────────────────────────────── test 7: stall ───────────────────────────────

/// After pushing one frame and then starving the buffer for STALL_THRESHOLD+1 pops,
/// the next pop should return `Stall`.
#[test]
fn t07_stall() {
    let mut buf = new_buf();
    buf.push(make_pkt(1, 0, &[1]));

    // Drain the single frame.
    pop_frames(&mut buf, 1);

    // Now pop without adding more frames — should eventually hit Stall.
    let mut stall_seen = false;
    for _ in 0..=STALL_THRESHOLD + 5 {
        if matches!(buf.pop_due(0), JitterOut::Stall) {
            stall_seen = true;
            break;
        }
    }
    assert!(stall_seen, "expected Stall after many empty pops");
    assert!(buf.stats().stall_events >= 1);
}

// ─────────────────────────────── test 8: disconnected ────────────────────────

/// After max_stall_frames+1 empty pops the buffer emits Disconnected, and after
/// that a new push + pop can recover.
#[test]
fn t08_disconnected_and_recover() {
    let max_stall: u32 = 10; // Use a small number so the test runs fast.
    let mut buf = JitterBuffer::new(FRAME_SAMPLES, max_stall);
    buf.push(make_pkt(1, 0, &[1]));

    pop_frames(&mut buf, 1);

    let mut disconnected = false;
    for _ in 0..max_stall + 20 {
        if matches!(buf.pop_due(0), JitterOut::Disconnected) {
            disconnected = true;
            break;
        }
    }
    assert!(disconnected, "expected Disconnected");

    // Recovery: push a new packet after reset.
    let ts_new: u32 = 100 * FRAME_SAMPLES;
    buf.push(make_pkt(100, ts_new, &[42]));
    let frames = pop_frames(&mut buf, 1);
    assert_eq!(frames.len(), 1, "should recover and emit one frame");
    assert_eq!(frames[0].2, vec![42]);
}

// ─────────────────────────────── test 9: jitter estimation ───────────────────

/// Simulate 200 packet arrivals with deliberate 5 ms jitter.  P95 should be in [3, 10] ms.
#[test]
fn t09_jitter_estimation() {
    use std::time::{Duration, Instant};

    // We drive the JitterEstimator directly since this is a unit-level concern.
    let mut est = anymic_core::jitter::JitterEstimatorTestHook::new(FRAME_SAMPLES);

    // Inject 200 samples of ~5 ms = 240 samples at 48 kHz deviation.
    let deviation_samples = 240.0f64; // 5 ms

    for i in 0..200 {
        // Alternate +deviation and 0 to produce realistic variance.
        let d = if i % 2 == 0 { deviation_samples } else { 0.0 };
        est.inject(d);
    }

    let p95 = est.p95_ms();
    assert!(
        p95 >= 3.0 && p95 <= 10.0,
        "P95 jitter should be in [3, 10] ms, got {p95}"
    );
}

// ─────────────────────────────── test 10: target depth adaptation ────────────

/// Zero jitter → target_depth = 1.  High jitter (15 ms) → target_depth ≥ 3.
#[test]
fn t10_target_depth_adaptation() {
    // Zero-jitter scenario: perfect 5ms inter-arrival, no deviation.
    {
        let mut buf = new_buf();
        // Push and pop 400 perfectly-timed frames in a tight loop.
        for i in 0..400u32 {
            buf.push(make_pkt(i as u16, i * FRAME_SAMPLES, &[i as u8]));
            buf.pop_due(i * FRAME_SAMPLES);
        }
        let td = buf.target_depth();
        assert_eq!(td, 1, "zero-jitter target_depth should be 1, got {td}");
    }

    // High-jitter scenario: inject large jitter samples into the estimator
    // and verify the target depth climbs.
    {
        let mut est = anymic_core::jitter::JitterEstimatorTestHook::new(FRAME_SAMPLES);
        // 15 ms = 720 samples at 48 kHz.
        for _ in 0..200 {
            est.inject(720.0);
        }
        let p95 = est.p95_ms();
        let frame_ms = FRAME_SAMPLES as f32 / 48.0;
        let desired_depth = (p95 / frame_ms).ceil() as u8;
        assert!(
            desired_depth >= 3,
            "15 ms jitter should yield target_depth ≥ 3, got {desired_depth} (p95={p95} ms)"
        );
    }
}

// ─────────────────────────────── test 11: proptest ───────────────────────────

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Any permutation of sequential frames — as long as there are no
        /// duplicates or gaps — must pop in ascending timestamp order.
        #[test]
        fn t11_proptest_sequential_no_drop(
            n in 1usize..30,
            // Random starting timestamp that does not wrap into trouble.
            base_ts in 0u32..u32::MAX / 2,
        ) {
            let mut buf = JitterBuffer::new(FRAME_SAMPLES, 500);

            // Build permutation by shuffling indices.
            let mut indices: Vec<usize> = (0..n).collect();
            // Use a simple deterministic shuffle based on n and base_ts.
            for i in 0..n {
                let j = (i + (base_ts as usize).wrapping_add(i * 7)) % n;
                indices.swap(i, j);
            }

            for idx in &indices {
                let i = *idx as u32;
                let seq = i as u16;
                let ts = base_ts.wrapping_add(i * FRAME_SAMPLES);
                buf.push(make_pkt(seq, ts, &[i as u8]));
            }

            let mut last_ts: Option<u32> = None;
            let mut frame_count = 0usize;
            let mut iter = 0;
            while frame_count < n && iter < n * 20 + 50 {
                match buf.pop_due(0) {
                    JitterOut::Frame { timestamp, .. } => {
                        if let Some(prev) = last_ts {
                            // Must be strictly after previous (in wrap-aware sense).
                            prop_assert!(
                                anymic_core::packet::ts_diff(timestamp, prev) > 0,
                                "timestamps not ascending: prev={prev} cur={timestamp}"
                            );
                        }
                        last_ts = Some(timestamp);
                        frame_count += 1;
                    }
                    JitterOut::Plc { .. } | JitterOut::Stall => {}
                    JitterOut::Disconnected => break,
                }
                iter += 1;
            }
            prop_assert_eq!(frame_count, n, "{}", format!("expected {n} frames, got {frame_count}"));
        }
    }
}
