//! Adaptive jitter buffer.
//!
//! # Overview
//!
//! Incoming [`RtpPacket`]s are stored in a [`BTreeMap`] keyed by timestamp.
//! A caller-driven clock (`pop_due`) advances one frame at a time; the buffer
//! decides whether to emit a real frame, a PLC placeholder, a stall event, or
//! a disconnected signal.
//!
//! # Jitter estimation
//!
//! Follows RFC 3550 §A.8: exponential moving average of inter-arrival
//! deviations with coefficient 1/16.  The running value is kept in 48 kHz
//! sample units and converted to milliseconds on read.
//!
//! # Target depth adaptation
//!
//! Every 200 frames the P95 of the last 200 jitter samples is recomputed and
//! the target depth is re-derived as `ceil(p95_ms / frame_ms)`, clamped to
//! `[1, 10]`.  To avoid oscillation the target moves by at most ±1 per epoch.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::packet::RtpPacket;

// ─────────────────────────────── constants ───────────────────────────────────

/// How many frames between target-depth recalculations.
const ADAPT_EPOCH: u32 = 200;

/// Number of jitter samples kept for the sliding-window P95.
const JITTER_WINDOW: usize = 200;

/// Consecutive missing pops that trigger `Stall` (≈ 250 ms at 5 ms/frame).
const STALL_THRESHOLD: u32 = 50;

/// RFC 3550 EMA coefficient denominator (1 / JITTER_EMA_DIV).
const JITTER_EMA_DIV: f64 = 16.0;

/// Minimum target depth in frames (20 frames = 100 ms — diagnostic stats
/// showed Android AudioRecord returns frames in bursts (5–10 frames at once
/// then 30–50 ms gap), causing ~30 PLC events/second even at a 50 ms buffer.
/// 100 ms covers typical burst patterns at the cost of additional latency.
const MIN_TARGET: u8 = 20;

/// Maximum target depth in frames (32 frames = 160 ms — adaptive algorithm
/// headroom for poor networks).
const MAX_TARGET: u8 = 32;

// ─────────────────────────────── public types ────────────────────────────────

/// Output of a single [`JitterBuffer::pop_due`] call.
pub enum JitterOut {
    /// A frame for the requested timestamp was buffered; emit it.
    Frame {
        seq: u16,
        timestamp: u32,
        payload: Vec<u8>,
    },
    /// No frame for this timestamp; caller should run PLC for `samples` samples.
    Plc { samples: u32 },
    /// Many consecutive frames were missing; stay silent.
    Stall,
    /// Prolonged absence of data; treat the session as lost.
    Disconnected,
}

/// Snapshot statistics from a [`JitterBuffer`].
#[derive(Debug, Clone, Default)]
pub struct JitterStats {
    /// Total packets accepted via [`JitterBuffer::push`].
    pub received: u64,
    /// Packets dropped because they arrived after their timestamp was already popped.
    pub late_dropped: u64,
    /// Packets dropped because the same timestamp was already in the buffer.
    pub duplicates: u64,
    /// PLC events emitted.
    pub plc_emitted: u64,
    /// Stall events emitted.
    pub stall_events: u64,
    /// Number of frames currently waiting in the buffer.
    pub current_depth: u8,
    /// Current adaptive target depth in frames.
    pub target_depth: u8,
    /// Estimated P95 jitter in milliseconds.
    pub jitter_ms_p95: f32,
}

// ─────────────────────────────── internal types ──────────────────────────────

/// A frame stored in the buffer (payload owned, header fields retained).
struct OwnedFrame {
    seq: u16,
    payload: Vec<u8>,
}

// ─────────────────────────────── JitterEstimator ─────────────────────────────

/// Jitter estimator following RFC 3550 §A.8.
///
/// Maintains an EMA of inter-arrival deviations in 48 kHz sample units.
pub struct JitterEstimator {
    /// RFC 3550 running jitter (fixed-point EMA).
    jitter_fp: f64,
    /// Sender timestamp of the last received packet.
    last_ts: Option<u32>,
    /// Wall-clock time the last packet arrived.
    last_arrival: Option<Instant>,
    /// Sliding window of recent absolute deviation samples (in 48 kHz samples).
    window: Vec<f64>,
    /// Write cursor into the sliding window.
    cursor: usize,
    /// True once the window has been filled at least once.
    full: bool,
}

impl JitterEstimator {
    fn new() -> Self {
        Self {
            jitter_fp: 0.0,
            last_ts: None,
            last_arrival: None,
            window: vec![0.0; JITTER_WINDOW],
            cursor: 0,
            full: false,
        }
    }

    /// Record a packet arrival.  `ts` is the sender's timestamp in samples.
    fn update(&mut self, ts: u32, now: Instant) {
        if let (Some(last_ts), Some(last_arrival)) = (self.last_ts, self.last_arrival) {
            // Expected inter-arrival in seconds (sender-side).
            let ts_delta_samples = crate::packet::ts_diff(ts, last_ts).max(0) as f64;
            let ts_delta_secs = ts_delta_samples / 48_000.0;

            // Actual wall-clock inter-arrival.
            let arrival_delta_secs = now.duration_since(last_arrival).as_secs_f64();

            // Absolute deviation in samples.
            let deviation_samples = ((arrival_delta_secs - ts_delta_secs) * 48_000.0).abs();

            // RFC 3550 EMA: J ← J + (|d| − J) / 16
            self.jitter_fp += (deviation_samples - self.jitter_fp) / JITTER_EMA_DIV;

            // Store in sliding window.
            self.record_sample(deviation_samples);
        }

        self.last_ts = Some(ts);
        self.last_arrival = Some(now);
    }

    /// Store a raw deviation sample into the sliding window (also updates EMA).
    fn record_sample(&mut self, deviation_samples: f64) {
        self.window[self.cursor] = deviation_samples;
        self.cursor = (self.cursor + 1) % JITTER_WINDOW;
        if self.cursor == 0 {
            self.full = true;
        }
    }

    /// Inject an artificial deviation sample (for testing only).
    pub fn inject_sample(&mut self, deviation_samples: f64) {
        self.jitter_fp += (deviation_samples - self.jitter_fp) / JITTER_EMA_DIV;
        self.record_sample(deviation_samples);
    }

    /// P95 of the sliding window, in milliseconds.
    pub fn p95_ms(&self) -> f32 {
        let len = if self.full {
            JITTER_WINDOW
        } else {
            self.cursor
        };
        if len == 0 {
            return 0.0;
        }

        let mut sorted: Vec<f64> = self.window[..len].to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // P95 index: ceiling of 0.95 * n, minus 1 (0-based), clamped.
        let idx = ((len as f64 * 0.95).ceil() as usize).saturating_sub(1);
        let idx = idx.min(len - 1);
        (sorted[idx] / 48.0) as f32
    }
}

// ─────────────────────────────── JitterBuffer ────────────────────────────────

/// Adaptive jitter buffer.
///
/// See the [module documentation](self) for a full description of the algorithm.
pub struct JitterBuffer {
    /// Stored frames, keyed by timestamp.
    frames: BTreeMap<u32, OwnedFrame>,
    /// Duration of a single audio frame in 48 kHz samples.
    frame_samples: u32,
    /// After this many consecutive no-frame pops the buffer becomes Disconnected.
    max_stall_frames: u32,

    // ── playback state ────────────────────────────────────────────────────────
    /// The timestamp we will pop next.  `None` until the first frame arrives.
    next_pop_ts: Option<u32>,
    /// The last timestamp that was popped (Frame or PLC).
    last_popped_ts: Option<u32>,
    /// Consecutive pop calls that did not yield a real Frame.
    consecutive_no_frame: u32,

    // ── startup warmup ────────────────────────────────────────────────────────
    /// True while we are collecting frames before the first playback.
    warming_up: bool,

    // ── adaptation ────────────────────────────────────────────────────────────
    /// Current adaptive target depth in frames.
    target_depth: u8,
    /// Frames processed since the last target-depth recalculation.
    epoch_counter: u32,
    /// Jitter estimator.
    estimator: JitterEstimator,

    // ── stats ─────────────────────────────────────────────────────────────────
    received: u64,
    late_dropped: u64,
    duplicates: u64,
    plc_emitted: u64,
    stall_events: u64,
}

impl JitterBuffer {
    // ── constructor ───────────────────────────────────────────────────────────

    /// Create a new jitter buffer.
    ///
    /// * `frame_samples` – 48 kHz samples per frame (e.g. 240 for 5 ms).
    /// * `max_stall_frames` – consecutive missing frames before `Disconnected`
    ///   (e.g. 500 ≈ 2.5 s at 5 ms/frame).
    pub fn new(frame_samples: u32, max_stall_frames: u32) -> Self {
        Self {
            frames: BTreeMap::new(),
            frame_samples,
            max_stall_frames,
            next_pop_ts: None,
            last_popped_ts: None,
            consecutive_no_frame: 0,
            warming_up: true,
            target_depth: MIN_TARGET,
            epoch_counter: 0,
            estimator: JitterEstimator::new(),
            received: 0,
            late_dropped: 0,
            duplicates: 0,
            plc_emitted: 0,
            stall_events: 0,
        }
    }

    // ── push ──────────────────────────────────────────────────────────────────

    /// Accept an incoming packet.
    ///
    /// Packets that arrive too late or are duplicates are silently counted and
    /// discarded.
    pub fn push(&mut self, pkt: RtpPacket<'_>) {
        let ts = pkt.timestamp;
        let now = Instant::now();

        // Update jitter estimator.
        self.estimator.update(ts, now);

        // Late check: arrived at or before the last popped ts.
        if let Some(last) = self.last_popped_ts {
            if crate::packet::ts_diff(ts, last) <= 0 {
                self.late_dropped += 1;
                return;
            }
        }

        // Duplicate check: same timestamp already buffered.
        if self.frames.contains_key(&ts) {
            self.duplicates += 1;
            return;
        }

        self.frames.insert(
            ts,
            OwnedFrame {
                seq: pkt.seq,
                payload: pkt.payload.into_owned(),
            },
        );
        self.received += 1;

        // On the very first packet: anchor next_pop_ts to the earliest buffered ts.
        // We always anchor to the BTreeMap minimum so out-of-order arrivals during
        // warmup start playback from the earliest frame, not whichever arrived first.
        if self.next_pop_ts.is_none() || self.warming_up {
            // During warmup, keep next_pop_ts pointed at the earliest buffered frame
            // so we don't skip frames that arrived out-of-order.
            if let Some(&earliest_ts) = self.frames.keys().next() {
                self.next_pop_ts = Some(earliest_ts);
            }
        }
    }

    // ── pop_due ───────────────────────────────────────────────────────────────

    /// Advance the playback clock by one frame and return what to emit.
    ///
    /// Call this once per 5 ms tick.  `now_samples` is available for external
    /// clock integration but the buffer manages its own cursor internally.
    pub fn pop_due(&mut self, _now_samples: u32) -> JitterOut {
        // ── Phase 0: no data ever arrived ────────────────────────────────────
        if self.next_pop_ts.is_none() {
            return self.no_frame_event();
        }

        // ── Phase 1: warmup — wait for target_depth frames ────────────────────
        if self.warming_up {
            if self.frames.len() < self.target_depth as usize {
                // Not enough frames buffered yet — emit PLC and don't advance.
                self.plc_emitted += 1;
                self.consecutive_no_frame += 1;
                return self.check_stall_or_plc();
            }
            // Enough frames: leave warmup and start popping.
            self.warming_up = false;
        }

        let next_ts = self.next_pop_ts.unwrap(); // safe: checked above

        // ── Phase 2: try to pop the frame at next_ts ──────────────────────────
        if let Some(frame) = self.frames.remove(&next_ts) {
            let out = JitterOut::Frame {
                seq: frame.seq,
                timestamp: next_ts,
                payload: frame.payload,
            };
            self.last_popped_ts = Some(next_ts);
            self.advance_pop_ts();
            self.consecutive_no_frame = 0;
            self.tick_epoch();
            return out;
        }

        // ── Phase 3: frame missing — emit PLC and advance ─────────────────────
        self.last_popped_ts = Some(next_ts);
        self.advance_pop_ts();
        self.consecutive_no_frame += 1;
        self.tick_epoch();
        self.plc_emitted += 1;
        self.check_stall_or_plc()
    }

    // ── accessors ─────────────────────────────────────────────────────────────

    /// Current adaptive target depth.
    pub fn target_depth(&self) -> u8 {
        self.target_depth
    }

    /// Snapshot of current statistics.
    pub fn stats(&self) -> JitterStats {
        JitterStats {
            received: self.received,
            late_dropped: self.late_dropped,
            duplicates: self.duplicates,
            plc_emitted: self.plc_emitted,
            stall_events: self.stall_events,
            current_depth: self.frames.len().min(u8::MAX as usize) as u8,
            target_depth: self.target_depth,
            jitter_ms_p95: self.estimator.p95_ms(),
        }
    }

    /// Reset playback state (called internally on Disconnected; also public).
    pub fn reset(&mut self) {
        self.frames.clear();
        self.next_pop_ts = None;
        self.last_popped_ts = None;
        self.consecutive_no_frame = 0;
        self.warming_up = true;
        self.epoch_counter = 0;
    }

    // ── private helpers ───────────────────────────────────────────────────────

    /// Called when no frame was available; returns Stall or Disconnected as
    /// warranted, otherwise delegates to the caller's PLC branch.
    ///
    /// NOTE: `plc_emitted` must be incremented by the caller *before* calling
    /// this when a PLC is being issued (so the stat is correct regardless of
    /// the escalation path).
    fn check_stall_or_plc(&mut self) -> JitterOut {
        if self.consecutive_no_frame > self.max_stall_frames {
            self.reset();
            return JitterOut::Disconnected;
        }
        if self.consecutive_no_frame > STALL_THRESHOLD {
            self.stall_events += 1;
            return JitterOut::Stall;
        }
        JitterOut::Plc {
            samples: self.frame_samples,
        }
    }

    /// Emit a no-frame event before any data has arrived.
    fn no_frame_event(&mut self) -> JitterOut {
        self.consecutive_no_frame += 1;
        // Don't count these as PLC — there's simply nothing yet.
        if self.consecutive_no_frame > self.max_stall_frames {
            self.reset();
            return JitterOut::Disconnected;
        }
        if self.consecutive_no_frame > STALL_THRESHOLD {
            self.stall_events += 1;
            return JitterOut::Stall;
        }
        JitterOut::Plc {
            samples: self.frame_samples,
        }
    }

    /// Advance `next_pop_ts` by one frame.
    fn advance_pop_ts(&mut self) {
        if let Some(ts) = self.next_pop_ts {
            self.next_pop_ts = Some(ts.wrapping_add(self.frame_samples));
        }
    }

    /// Tick the epoch counter; recalculate target depth every ADAPT_EPOCH pops.
    fn tick_epoch(&mut self) {
        self.epoch_counter += 1;
        if self.epoch_counter >= ADAPT_EPOCH {
            self.epoch_counter = 0;
            self.recalculate_target();
        }
    }

    /// Recompute target depth from P95 jitter, moving at most ±1 from current.
    fn recalculate_target(&mut self) {
        let p95_ms = self.estimator.p95_ms();
        let frame_ms = (self.frame_samples as f32) / 48.0;
        let raw = (p95_ms / frame_ms).ceil() as u8;
        let desired = raw.clamp(MIN_TARGET, MAX_TARGET);

        // Smooth: move at most ±1 per epoch to avoid oscillation.
        self.target_depth = if desired > self.target_depth {
            (self.target_depth + 1).min(MAX_TARGET)
        } else if desired < self.target_depth {
            self.target_depth.saturating_sub(1).max(MIN_TARGET)
        } else {
            desired
        };
    }
}

// ─────────────────────────────── test hook ───────────────────────────────────

/// Public wrapper around [`JitterEstimator`] for integration tests.
///
/// This type is `#[doc(hidden)]` and should not be relied on by application code.
#[doc(hidden)]
pub struct JitterEstimatorTestHook(JitterEstimator);

impl JitterEstimatorTestHook {
    /// Create a new estimator.
    pub fn new(_frame_samples: u32) -> Self {
        Self(JitterEstimator::new())
    }

    /// Inject a raw deviation in 48 kHz samples.
    pub fn inject(&mut self, deviation_samples: f64) {
        self.0.inject_sample(deviation_samples);
    }

    /// P95 jitter in milliseconds.
    pub fn p95_ms(&self) -> f32 {
        self.0.p95_ms()
    }
}

// ─────────────────────────────── unit tests ──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{Flags, PayloadType, RtpPacket};
    use std::borrow::Cow;

    const FRAME_SAMPLES: u32 = 240;

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

    #[test]
    fn estimator_p95_empty() {
        let est = JitterEstimator::new();
        assert_eq!(est.p95_ms(), 0.0);
    }

    #[test]
    fn estimator_p95_single_sample() {
        let mut est = JitterEstimator::new();
        est.inject_sample(480.0); // 10 ms deviation
        assert!(est.p95_ms() > 0.0);
    }

    #[test]
    fn target_depth_clamped_min() {
        let buf = JitterBuffer::new(FRAME_SAMPLES, 500);
        assert_eq!(buf.target_depth(), MIN_TARGET);
    }

    #[test]
    fn push_sets_next_pop_ts_to_first_frame() {
        let mut buf = JitterBuffer::new(FRAME_SAMPLES, 500);
        buf.push(make_pkt(1, 0, &[1]));
        // next_pop_ts should be 0 (the first frame's ts)
        assert_eq!(buf.next_pop_ts, Some(0));
    }

    #[test]
    fn warmup_anchors_to_earliest_ts() {
        let mut buf = JitterBuffer::new(FRAME_SAMPLES, 500);
        // Push out-of-order: ts=480 first, then ts=0, ts=240.
        buf.push(make_pkt(3, 480, &[3]));
        buf.push(make_pkt(1, 0, &[1]));
        buf.push(make_pkt(2, 240, &[2]));
        // During warmup the anchor re-evaluates on each push → should be 0.
        assert_eq!(buf.next_pop_ts, Some(0));
    }
}
