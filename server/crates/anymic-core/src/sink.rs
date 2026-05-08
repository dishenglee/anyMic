//! Platform-agnostic audio sink that the server pipeline writes decoded PCM into.

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("device unavailable: {0}")]
    Unavailable(String),
    #[error("os error: {0}")]
    Os(String),
    #[error("buffer underrun")]
    Underrun,
}

#[derive(Debug, Clone)]
pub struct SinkInfo {
    /// Display name, e.g. "BlackHole 2ch"
    pub name: String,
    /// Platform-stable device ID (macOS: device UID; Win/Linux implementations define their own)
    pub uid: String,
    /// Should be 48000
    pub sample_rate: u32,
    /// 1 or 2 (2ch device internally duplicates mono → L/R)
    pub channels: u16,
}

pub trait AudioSink: Send {
    /// Write one frame of PCM s16 mono (48 kHz). If the device is 2ch, the implementation
    /// internally duplicates the signal to stereo.
    ///
    /// Callers feed frames on a 5 ms cadence (240 samples). The implementation may buffer
    /// a number of frames internally but must not block for more than 10 ms.
    fn write_mono_s16(&mut self, pcm: &[i16]) -> Result<(), SinkError>;

    fn info(&self) -> &SinkInfo;

    /// At session end, push silence to avoid the last frame being repeated by the device.
    fn drain_silence(&mut self, duration_ms: u32);

    /// Number of buffer underruns observed by the sink (e.g. CoreAudio render
    /// callback fired with insufficient data).  Default 0 if the sink doesn't
    /// track this.
    fn underrun_count(&self) -> u64 {
        0
    }
}
