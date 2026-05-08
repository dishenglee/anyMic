//! Opus frame decoder with PLC.
//!
//! Decodes Opus-encoded audio frames into mono 16-bit PCM samples.
//! Supports packet loss concealment (PLC) for graceful handling of
//! dropped frames in real-time voice streaming.

use audiopus::coder::GenericCtl;
use audiopus::{coder::Decoder as AudiopusDecoder, Channels, MutSignals, SampleRate};
use std::convert::TryFrom;
use thiserror::Error;

/// Errors that can arise from the Opus frame decoder.
#[derive(Debug, Error)]
pub enum DecoderError {
    #[error("opus decode failed: {0}")]
    Opus(String),
    #[error("invalid frame size: got {got} samples, expected {expected}")]
    InvalidFrameSize { got: usize, expected: usize },
}

impl From<audiopus::Error> for DecoderError {
    fn from(e: audiopus::Error) -> Self {
        DecoderError::Opus(e.to_string())
    }
}

/// Sink-agnostic, frame-agnostic decoder trait.
pub trait FrameDecoder: Send {
    /// Decode one Opus frame. Returns mono PCM s16.
    fn decode(&mut self, opus_payload: &[u8]) -> Result<Vec<i16>, DecoderError>;

    /// PLC: synthesize `samples` worth of extrapolation when a frame is lost.
    ///
    /// Passes `None` as the packet to `opus_decode`, which triggers Opus's
    /// built-in packet-loss concealment path.
    fn decode_plc(&mut self, samples: usize) -> Vec<i16>;

    /// Reset internal state (call on session reconnect).
    fn reset(&mut self);
}

/// Opus frame decoder with configurable sample rate, channel count, and frame
/// size. Implements `FrameDecoder` and exposes PLC via the libopus concealment
/// path (`decode(None, ...)`).
pub struct OpusFrameDecoder {
    inner: AudiopusDecoder,
    sample_rate: u32,
    channels: u16,
    /// Expected number of PCM samples per frame (e.g. 240 for 5 ms @ 48 kHz mono).
    frame_samples: usize,
}

impl OpusFrameDecoder {
    /// Create a decoder with the default VoIP profile:
    /// 48 kHz / mono / 5 ms frame (240 samples).
    pub fn new_voip() -> Result<Self, DecoderError> {
        Self::with_params(48_000, 1, 240)
    }

    /// Create a decoder with explicit parameters.
    ///
    /// `sample_rate` must be one of 8000, 12000, 16000, 24000, 48000.
    /// `channels` must be 1 (mono) or 2 (stereo).
    /// `frame_samples` is the number of PCM samples per frame (per channel).
    pub fn with_params(
        sample_rate: u32,
        channels: u16,
        frame_samples: usize,
    ) -> Result<Self, DecoderError> {
        let sr = match sample_rate {
            8_000 => SampleRate::Hz8000,
            12_000 => SampleRate::Hz12000,
            16_000 => SampleRate::Hz16000,
            24_000 => SampleRate::Hz24000,
            48_000 => SampleRate::Hz48000,
            other => {
                return Err(DecoderError::Opus(format!(
                    "unsupported sample rate: {other}"
                )))
            }
        };

        let ch = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            other => {
                return Err(DecoderError::Opus(format!(
                    "unsupported channel count: {other}"
                )))
            }
        };

        let inner = AudiopusDecoder::new(sr, ch)?;

        Ok(Self {
            inner,
            sample_rate,
            channels,
            frame_samples,
        })
    }
}

impl FrameDecoder for OpusFrameDecoder {
    fn decode(&mut self, opus_payload: &[u8]) -> Result<Vec<i16>, DecoderError> {
        use audiopus::packet::Packet;

        // Allocate output buffer: frame_samples per channel.
        let buf_len = self.frame_samples * self.channels as usize;
        let mut buf = vec![0i16; buf_len];

        let packet =
            Packet::try_from(opus_payload).map_err(|e| DecoderError::Opus(e.to_string()))?;

        let output = MutSignals::try_from(buf.as_mut_slice())
            .map_err(|e| DecoderError::Opus(e.to_string()))?;

        let decoded = self
            .inner
            .decode(Some(packet), output, false)
            .map_err(|e| DecoderError::Opus(e.to_string()))?;

        // `decoded` is the number of samples per channel.
        if decoded != self.frame_samples {
            return Err(DecoderError::InvalidFrameSize {
                got: decoded,
                expected: self.frame_samples,
            });
        }

        buf.truncate(decoded * self.channels as usize);
        Ok(buf)
    }

    fn decode_plc(&mut self, samples: usize) -> Vec<i16> {
        let buf_len = samples * self.channels as usize;
        let mut buf = vec![0i16; buf_len];

        let output = match MutSignals::try_from(buf.as_mut_slice()) {
            Ok(o) => o,
            Err(_) => return buf,
        };

        // Passing `None` as the packet triggers Opus PLC.
        match self.inner.decode(None, output, false) {
            Ok(n) => {
                buf.truncate(n * self.channels as usize);
            }
            Err(_) => {
                // On PLC failure, return zero-filled silence.
                buf.truncate(buf_len);
            }
        }

        buf
    }

    fn reset(&mut self) {
        // Ignore errors: reset_state should always succeed on a valid decoder.
        let _ = self.inner.reset_state();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_voip_does_not_error() {
        OpusFrameDecoder::new_voip().expect("new_voip should succeed");
    }

    #[test]
    fn with_params_bad_sample_rate_errors() {
        let err = OpusFrameDecoder::with_params(44_100, 1, 220);
        assert!(err.is_err(), "44100 Hz should be rejected");
    }
}
