//! Windows audio sink — writes PCM to the VB-Audio Virtual Cable input device
//! via WASAPI (through cpal).
//!
//! Architecture:
//!     write_mono_s16()   ─→ [ring buffer] ─→ cpal callback ─→ VB-CABLE Input
//!  [caller thread]                            [audio thread]
//!
//! VB-CABLE is a loopback virtual device: applications that select
//! "CABLE Output (VB-Audio Virtual Cable)" as their **input** source will
//! hear whatever we write to the **CABLE Input** render endpoint.

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use anymic_core::sink::{AudioSink, SinkError, SinkInfo};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use tracing::{info, warn};

    // ── ring buffer ───────────────────────────────────────────────────────────

    /// Lock-free SPSC ring buffer for i16 stereo interleaved samples.
    /// Capacity must be a power of two.
    struct Ring {
        buf: Box<[i16]>,
        mask: usize,
        write: AtomicU64,
        read: AtomicU64,
    }

    impl Ring {
        fn new(capacity: usize) -> Arc<Self> {
            assert!(capacity.is_power_of_two());
            Arc::new(Ring {
                buf: vec![0i16; capacity].into_boxed_slice(),
                mask: capacity - 1,
                write: AtomicU64::new(0),
                read: AtomicU64::new(0),
            })
        }

        fn available_write(&self) -> usize {
            let w = self.write.load(Ordering::Acquire);
            let r = self.read.load(Ordering::Acquire);
            self.buf.len() - (w.wrapping_sub(r) as usize)
        }

        fn available_read(&self) -> usize {
            let w = self.write.load(Ordering::Acquire);
            let r = self.read.load(Ordering::Acquire);
            w.wrapping_sub(r) as usize
        }

        /// Push samples; returns how many were actually written.
        fn push(&self, data: &[i16]) -> usize {
            let space = self.available_write();
            let n = data.len().min(space);
            if n == 0 {
                return 0;
            }
            let w = self.write.load(Ordering::Relaxed) as usize;
            for (i, &s) in data[..n].iter().enumerate() {
                unsafe {
                    let ptr = self.buf.as_ptr().add((w + i) & self.mask) as *mut i16;
                    ptr.write(s);
                }
            }
            self.write.fetch_add(n as u64, Ordering::Release);
            n
        }

        /// Pop up to `out.len()` samples.
        fn pop(&self, out: &mut [i16]) -> usize {
            let avail = self.available_read();
            let n = out.len().min(avail);
            if n == 0 {
                return 0;
            }
            let r = self.read.load(Ordering::Relaxed) as usize;
            for (i, slot) in out[..n].iter_mut().enumerate() {
                *slot = unsafe { self.buf.as_ptr().add((r + i) & self.mask).read() };
            }
            self.read.fetch_add(n as u64, Ordering::Release);
            n
        }
    }

    unsafe impl Send for Ring {}
    unsafe impl Sync for Ring {}

    // ── device discovery ──────────────────────────────────────────────────────

    const DEVICE_CONTAINS: &str = "CABLE Input";
    const DEVICE_FULL_NAME: &str = "CABLE Input (VB-Audio Virtual Cable)";

    fn find_vbcable_device() -> Result<cpal::Device, SinkError> {
        let host = cpal::default_host();
        let devices = host.output_devices().map_err(|e| {
            SinkError::Os(format!("cpal output_devices: {e}"))
        })?;

        for device in devices {
            let name = device.name().unwrap_or_default();
            if name.contains(DEVICE_CONTAINS) {
                info!("found VB-CABLE device: {name}");
                return Ok(device);
            }
        }

        Err(SinkError::DeviceNotFound(format!(
            "{DEVICE_FULL_NAME} — install VB-CABLE from https://vb-audio.com/Cable/ and reboot"
        )))
    }

    // ── shared callback state ─────────────────────────────────────────────────

    struct CallbackState {
        ring: Arc<Ring>,
        underruns: AtomicU64,
    }

    unsafe impl Send for CallbackState {}
    unsafe impl Sync for CallbackState {}

    // ── public sink type ──────────────────────────────────────────────────────

    /// A sink that writes mono 16-bit PCM into the VB-Audio Virtual Cable
    /// input device (CABLE Input) via WASAPI through cpal.
    ///
    /// Construct via [`SystemVbCableSink::open`].
    pub struct SystemVbCableSink {
        /// The cpal stream — kept alive to drive the audio callback.
        _stream: cpal::Stream,
        ring: Arc<Ring>,
        callback_state: Arc<CallbackState>,
        info: SinkInfo,
        /// Most recent stereo-expanded frame; used for mono→stereo expansion
        /// and ring top-up.
        last_stereo_frame: Vec<i16>,
    }

    // cpal::Stream is not Send on Windows due to COM thread-affinity, but we
    // ensure it is only touched from the thread that created it (the server
    // init thread), so Send is safe here.
    unsafe impl Send for SystemVbCableSink {}

    impl SystemVbCableSink {
        /// Open the VB-CABLE input device and return a ready-to-use sink.
        ///
        /// Returns `Err(SinkError::DeviceNotFound)` if VB-CABLE is not installed.
        pub fn open() -> Result<Self, SinkError> {
            let device = find_vbcable_device()?;
            let device_name = device.name().unwrap_or_else(|_| DEVICE_FULL_NAME.to_string());

            // Negotiate a supported config close to 48 kHz stereo i16.
            // cpal on WASAPI may need float; we prefer i16 but fall back to f32.
            let config = Self::negotiate_config(&device)?;

            // Ring buffer: 16 384 stereo samples ≈ 170 ms at 48 kHz.
            let ring = Ring::new(16_384);

            // Prebuffer 100 ms of silence so the callback never underruns on
            // startup before the caller starts writing.
            let prebuffer_samples = (48_000usize * 100 / 1000) * 2; // 100 ms × 2 ch
            let _ = ring.push(&vec![0i16; prebuffer_samples]);

            let callback_state = Arc::new(CallbackState {
                ring: Arc::clone(&ring),
                underruns: AtomicU64::new(0),
            });

            let stream = Self::build_stream(&device, &config, Arc::clone(&callback_state))?;
            stream.play().map_err(|e| SinkError::Os(format!("stream play: {e}")))?;

            info!(device = %device_name, sample_rate = config.sample_rate.0, "VB-CABLE sink opened");

            let info = SinkInfo {
                name: device_name.clone(),
                uid: format!("vbcable-wasapi-{}", device_name),
                sample_rate: config.sample_rate.0,
                channels: 2,
            };

            Ok(SystemVbCableSink {
                _stream: stream,
                ring,
                callback_state,
                info,
                last_stereo_frame: Vec::with_capacity(2048),
            })
        }

        /// Find the best supported stream config: prefer i16 stereo 48 kHz,
        /// fall back to f32 or whatever the device offers.
        fn negotiate_config(device: &cpal::Device) -> Result<cpal::StreamConfig, SinkError> {
            // Try to get supported output configs and find one close to 48 kHz stereo.
            if let Ok(mut configs) = device.supported_output_configs() {
                // Prefer stereo (2 ch) at 48 kHz.
                let candidate = configs.find(|c| {
                    c.channels() == 2
                        && c.min_sample_rate().0 <= 48_000
                        && c.max_sample_rate().0 >= 48_000
                });
                if let Some(c) = candidate {
                    return Ok(c.with_sample_rate(cpal::SampleRate(48_000)).config());
                }
            }

            // Fall back to the device default config.
            device
                .default_output_config()
                .map(|c| c.config())
                .map_err(|e| SinkError::Os(format!("default_output_config: {e}")))
        }

        /// Build the cpal output stream.  We always write i16 stereo internally;
        /// if the device negotiated f32 we convert on the fly in the callback.
        fn build_stream(
            device: &cpal::Device,
            config: &cpal::StreamConfig,
            state: Arc<CallbackState>,
        ) -> Result<cpal::Stream, SinkError> {
            let channels = config.channels as usize;
            let err_fn = |e| warn!("cpal stream error: {e}");

            // Try i16 first; WASAPI in shared mode usually exposes f32 so we
            // may fall through.
            let stream_result = {
                let state2 = Arc::clone(&state);
                device.build_output_stream(
                    config,
                    move |out: &mut [i16], _| {
                        Self::fill_callback_i16(out, channels, &state2);
                    },
                    err_fn,
                    None,
                )
            };

            if let Ok(stream) = stream_result {
                return Ok(stream);
            }

            // Fall back: f32 output.  Convert our i16 ring data to f32 in-callback.
            let state3 = Arc::clone(&state);
            device
                .build_output_stream(
                    config,
                    move |out: &mut [f32], _| {
                        Self::fill_callback_f32(out, channels, &state3);
                    },
                    move |e| warn!("cpal stream error (f32 path): {e}"),
                    None,
                )
                .map_err(|e| SinkError::Os(format!("build_output_stream: {e}")))
        }

        /// Callback for i16 output: pop from ring, hold last sample on underrun.
        fn fill_callback_i16(out: &mut [i16], channels: usize, state: &CallbackState) {
            // Ensure we write in stereo pairs; if device is mono, duplicate.
            if channels >= 2 {
                let got = state.ring.pop(out);
                if got < out.len() {
                    // Hold last L/R sample to avoid click discontinuity.
                    let (last_l, last_r) = if got >= 2 {
                        (out[got - 2], out[got - 1])
                    } else {
                        (0i16, 0i16)
                    };
                    let mut i = got;
                    while i < out.len() {
                        out[i] = last_l;
                        if i + 1 < out.len() {
                            out[i + 1] = last_r;
                        }
                        i += 2;
                    }
                    state.underruns.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                // Mono device: pop only left channel samples.
                let needed = out.len();
                let mut tmp = vec![0i16; needed * 2];
                let got = state.ring.pop(&mut tmp[..needed * 2]);
                let last = if got >= 2 { tmp[got.saturating_sub(2)] } else { 0i16 };
                for (i, slot) in out.iter_mut().enumerate() {
                    *slot = if i * 2 < got { tmp[i * 2] } else { last };
                }
                if got < needed * 2 {
                    state.underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        /// Callback for f32 output: convert i16 ring samples to f32 [-1.0, 1.0].
        fn fill_callback_f32(out: &mut [f32], channels: usize, state: &CallbackState) {
            let needed_i16 = if channels >= 2 { out.len() } else { out.len() * 2 };
            let mut tmp = vec![0i16; needed_i16];
            let got = state.ring.pop(&mut tmp);

            let last_l = if got >= 2 { tmp[got - 2] } else { 0i16 };
            let last_r = if got >= 2 { tmp[got - 1] } else { 0i16 };

            if channels >= 2 {
                for (i, slot) in out.iter_mut().enumerate() {
                    let sample = if i < got {
                        tmp[i]
                    } else if i % 2 == 0 {
                        last_l
                    } else {
                        last_r
                    };
                    *slot = sample as f32 / 32768.0;
                }
            } else {
                for (i, slot) in out.iter_mut().enumerate() {
                    let sample = if i * 2 < got { tmp[i * 2] } else { last_l };
                    *slot = sample as f32 / 32768.0;
                }
            }

            if got < needed_i16 {
                state.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    impl AudioSink for SystemVbCableSink {
        fn write_mono_s16(&mut self, pcm: &[i16]) -> Result<(), SinkError> {
            // Expand mono → stereo interleaved.
            self.last_stereo_frame.clear();
            self.last_stereo_frame.reserve(pcm.len() * 2);
            for &s in pcm {
                self.last_stereo_frame.push(s);
                self.last_stereo_frame.push(s);
            }

            let written = self.ring.push(&self.last_stereo_frame);
            if written < self.last_stereo_frame.len() {
                return Err(SinkError::Underrun);
            }

            // Top-up: if ring level < 30 ms, repeat last frame (up to 10×) to
            // prevent the cpal callback from underrunning between server ticks.
            const TARGET_MS: u32 = 30;
            let target_samples: usize = 48 * TARGET_MS as usize * 2; // 48 samples/ms × 2 ch
            let mut topups = 0usize;
            while self.ring.available_read() < target_samples && topups < 10 {
                let _ = self.ring.push(&self.last_stereo_frame);
                topups += 1;
            }

            Ok(())
        }

        fn info(&self) -> &SinkInfo {
            &self.info
        }

        fn drain_silence(&mut self, duration_ms: u32) {
            let samples_mono = (48_000u32 * duration_ms / 1_000) as usize;
            let stereo_samples = samples_mono * 2;
            let silence = vec![0i16; stereo_samples];
            self.ring.push(&silence);
            std::thread::sleep(std::time::Duration::from_millis(duration_ms as u64 + 10));
        }

        fn underrun_count(&self) -> u64 {
            self.callback_state.underruns.load(Ordering::Relaxed)
        }
    }
}

// ── public re-exports ─────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub use windows_impl::SystemVbCableSink;

#[cfg(not(target_os = "windows"))]
pub fn placeholder() {}
