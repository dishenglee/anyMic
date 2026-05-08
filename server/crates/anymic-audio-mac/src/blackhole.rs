//! BlackHole 2ch sink — writes PCM into the BlackHole virtual audio device via CoreAudio AUHAL.
//!
//! # Architecture
//!
//! ```text
//!  write_mono_s16()          AURenderCallback (real-time thread)
//!  [caller thread]    ─────> [ring buffer] ──────────────────────>  BlackHole 2ch output
//! ```
//!
//! The AUHAL unit is opened in *output* mode pointed at the BlackHole device.
//! BlackHole is a loopback virtual device: applications that select "BlackHole 2ch" as their
//! **input** device will receive whatever is written to its **output** side.  So by treating it
//! as an output device here we are effectively injecting audio into the system input path.
//!
//! The render callback runs on a real-time CoreAudio thread at whatever buffer size CoreAudio
//! chooses (typically 256–512 samples at 48 kHz).  We use a SPSC ring buffer with capacity 50
//! frames × 240 samples = 12 000 stereo i16 samples (≈ 250 ms) to decouple the caller's 5 ms
//! cadence from the callback's cadence.  If the ring is empty when the callback fires we fill
//! with silence and count the underrun.

#![allow(non_upper_case_globals)]

use std::os::raw::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anymic_core::sink::{AudioSink, SinkError, SinkInfo};

// ─── ring buffer ─────────────────────────────────────────────────────────────

/// Simple SPSC ring buffer for i16 stereo interleaved samples.
///
/// Capacity in *samples* (not frames, not bytes).  Must be a power of 2.
struct Ring {
    buf: Box<[i16]>,
    mask: usize,
    /// write position (in samples, wraps via mask)
    write: AtomicU64,
    /// read position (in samples, wraps via mask)
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

    /// Available space (in samples).
    fn available_write(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire);
        self.buf.len() - (w.wrapping_sub(r) as usize)
    }

    /// Samples ready to read.
    fn available_read(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire);
        w.wrapping_sub(r) as usize
    }

    /// Push samples; returns how many were actually written (may be less if ring is full).
    fn push(&self, data: &[i16]) -> usize {
        let space = self.available_write();
        let n = data.len().min(space);
        if n == 0 {
            return 0;
        }
        let w = self.write.load(Ordering::Relaxed) as usize;
        for (i, &s) in data[..n].iter().enumerate() {
            // Safety: single writer, index within allocated slice.
            unsafe {
                let ptr = self.buf.as_ptr().add((w + i) & self.mask) as *mut i16;
                ptr.write(s);
            }
        }
        self.write.fetch_add(n as u64, Ordering::Release);
        n
    }

    /// Pop up to `out.len()` samples.  Returns how many were actually read.
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

// ─── CoreAudio imports ────────────────────────────────────────────────────────

use coreaudio::audio_unit::macos_helpers::{get_audio_device_ids, get_device_name};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, Scope};
use coreaudio::sys::{
    kAudioFormatFlagIsSignedInteger, kAudioFormatFlagsNativeEndian, kAudioFormatLinearPCM,
    kAudioOutputUnitProperty_CurrentDevice, kAudioOutputUnitProperty_EnableIO,
    kAudioUnitProperty_SetRenderCallback, kAudioUnitProperty_StreamFormat, AURenderCallbackStruct,
    AudioBufferList, AudioComponentInstanceDispose, AudioDeviceID, AudioOutputUnitStart,
    AudioOutputUnitStop, AudioStreamBasicDescription, AudioTimeStamp, AudioUnitInitialize,
    AudioUnitRenderActionFlags, AudioUnitSetProperty, AudioUnitUninitialize,
};

// ─── device lookup ────────────────────────────────────────────────────────────

const DEVICE_NAME: &str = "BlackHole 2ch";

fn find_blackhole_device() -> Result<AudioDeviceID, SinkError> {
    let ids = get_audio_device_ids()
        .map_err(|e| SinkError::Os(format!("AudioObjectGetPropertyData: {e:?}")))?;

    for id in ids {
        if let Ok(name) = get_device_name(id) {
            if name == DEVICE_NAME {
                return Ok(id);
            }
        }
    }
    Err(SinkError::DeviceNotFound(DEVICE_NAME.to_string()))
}

// ─── callback state ───────────────────────────────────────────────────────────

/// State shared between the caller and the real-time render callback.
struct CallbackState {
    ring: Arc<Ring>,
    underruns: AtomicU64,
}

unsafe impl Send for CallbackState {}
unsafe impl Sync for CallbackState {}

// ─── AUHAL render callback ────────────────────────────────────────────────────
//
// We register this as a raw C function pointer instead of going through
// coreaudio-rs's `set_render_callback`, because that helper enforces a type
// check against the stream format that rejects i16 interleaved on macOS
// (the mac "canonical" format is f32).  Using `set_property` directly lets us
// supply any valid ASBD.

extern "C" fn render_callback(
    in_ref_con: *mut c_void,
    _io_action_flags: *mut AudioUnitRenderActionFlags,
    _in_time_stamp: *const AudioTimeStamp,
    _in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList,
) -> i32 {
    let state = unsafe { &*(in_ref_con as *const CallbackState) };
    let frames = in_number_frames as usize;
    let stereo_samples = frames * 2; // 2 channels interleaved

    // Safety: CoreAudio owns io_data; buffer layout matches our ASBD (1 interleaved buffer).
    // io_data is non-null when inputProc is registered for output rendering.
    let out_slice: &mut [i16] = unsafe {
        let buffer = &mut (*io_data).mBuffers[0];
        let ptr = buffer.mData as *mut i16;
        std::slice::from_raw_parts_mut(ptr, stereo_samples)
    };

    let got = state.ring.pop(out_slice);
    if got < stereo_samples {
        // Underrun: hold the last L/R sample values instead of dropping to zero.
        // Plain silence at the boundary produces an audible click when a real
        // signal was playing; holding the last DC value avoids the
        // discontinuity.  The held value is naturally overwritten by the next
        // ring push, so the signal recovers smoothly.
        let (last_l, last_r) = if got >= 2 {
            (out_slice[got - 2], out_slice[got - 1])
        } else {
            (0i16, 0i16)
        };
        let mut i = got;
        while i < stereo_samples {
            out_slice[i] = last_l;
            if i + 1 < stereo_samples {
                out_slice[i + 1] = last_r;
            }
            i += 2;
        }
        state.underruns.fetch_add(1, Ordering::Relaxed);
    }
    0 // noErr
}

// ─── public sink type ─────────────────────────────────────────────────────────

/// A sink that writes mono 16-bit PCM into the BlackHole 2ch virtual device.
///
/// Construct via [`SystemBlackHoleSink::open`].
pub struct SystemBlackHoleSink {
    /// Raw AudioUnit instance — we manage lifecycle manually to avoid drop hangs.
    au_instance: coreaudio::sys::AudioUnit,
    /// CallbackState is heap-allocated and pointed to by the render callback.
    /// MUST outlive `au_instance` (i.e., dropped after we clear the callback and stop).
    callback_state: Option<Box<CallbackState>>,
    ring: Arc<Ring>,
    info: SinkInfo,
    /// Most recent stereo-expanded frame; used to top up the ring buffer when
    /// it drains below the safe threshold, preventing render-callback underruns
    /// caused by clock drift between server tick and CoreAudio callback.
    last_stereo_frame: Vec<i16>,
}

// Safety: AudioUnit is a pointer-sized opaque type. We manage all access from
// a single non-RT thread (the caller thread). The real-time callback thread
// only touches the CallbackState through a separate pointer.
unsafe impl Send for SystemBlackHoleSink {}

impl SystemBlackHoleSink {
    /// Open the BlackHole 2ch device and return a ready-to-use sink.
    ///
    /// Returns `Err` if BlackHole 2ch is not installed.
    pub fn open() -> Result<Self, SinkError> {
        let device_id = find_blackhole_device()?;

        // Build an AudioUnit via coreaudio-rs, then immediately extract the raw instance
        // and mem::forget the wrapper to prevent double-cleanup via its Drop.
        let au_wrapper = AudioUnit::new(IOType::HalOutput)
            .map_err(|e| SinkError::Os(format!("AudioUnit::new: {e:?}")))?;
        let au_instance = *au_wrapper.as_ref();
        // Prevent AudioUnit::drop from running — we own the lifecycle from here.
        std::mem::forget(au_wrapper);

        // A macro to check OSStatus and return a SinkError on failure.
        macro_rules! au_check {
            ($status:expr, $msg:expr) => {{
                let s = $status;
                if s != 0 {
                    return Err(SinkError::Os(format!("{}: OSStatus {}", $msg, s)));
                }
            }};
        }

        unsafe {
            // Point the unit at the BlackHole device.
            au_check!(
                AudioUnitSetProperty(
                    au_instance,
                    kAudioOutputUnitProperty_CurrentDevice,
                    Scope::Global as u32,
                    Element::Output as u32,
                    &device_id as *const AudioDeviceID as *const c_void,
                    std::mem::size_of::<AudioDeviceID>() as u32,
                ),
                "SetCurrentDevice"
            );

            // Disable input bus, keep output bus enabled.
            let disable: u32 = 0u32;
            let enable: u32 = 1u32;
            au_check!(
                AudioUnitSetProperty(
                    au_instance,
                    kAudioOutputUnitProperty_EnableIO,
                    Scope::Input as u32,
                    Element::Input as u32,
                    &disable as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                ),
                "DisableInput"
            );
            au_check!(
                AudioUnitSetProperty(
                    au_instance,
                    kAudioOutputUnitProperty_EnableIO,
                    Scope::Output as u32,
                    Element::Output as u32,
                    &enable as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                ),
                "EnableOutput"
            );

            // Stream format: 48 kHz, s16 interleaved stereo.
            // kAudioFormatFlagIsPacked = 8 (standard CoreAudio constant).
            const kAudioFormatFlagIsPacked: u32 = 8;
            let asbd = AudioStreamBasicDescription {
                mSampleRate: 48_000.0,
                mFormatID: kAudioFormatLinearPCM,
                mFormatFlags: kAudioFormatFlagsNativeEndian
                    | kAudioFormatFlagIsSignedInteger
                    | kAudioFormatFlagIsPacked,
                mBytesPerPacket: 4, // 2 ch × 2 bytes
                mFramesPerPacket: 1,
                mBytesPerFrame: 4,
                mChannelsPerFrame: 2,
                mBitsPerChannel: 16,
                mReserved: 0,
            };
            // "Input" scope of the Output element = the data we push in.
            au_check!(
                AudioUnitSetProperty(
                    au_instance,
                    kAudioUnitProperty_StreamFormat,
                    Scope::Input as u32,
                    Element::Output as u32,
                    &asbd as *const AudioStreamBasicDescription as *const c_void,
                    std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                ),
                "SetStreamFormat"
            );

            // Ring buffer: 16 384 stereo samples ≈ 170 ms at 48 kHz.
            let ring = Ring::new(16_384);

            // Prebuffer 200 ms of silence to absorb both tokio tick jitter
            // and the natural drift between the server's 5 ms tick clock and
            // the CoreAudio render callback's irregular cadence.  Diagnostic
            // logs at 100 ms still showed ~1–3 underruns/sec, so we double it.
            let prebuffer_samples = (48_000_usize * 200 / 1000) * 2; // 200 ms × 2 channels
            let _ = ring.push(&vec![0i16; prebuffer_samples]);

            // Callback state allocated on the heap — pointer survives across callback invocations.
            let callback_state = Box::new(CallbackState {
                ring: Arc::clone(&ring),
                underruns: AtomicU64::new(0),
            });
            let cb_ptr = &*callback_state as *const CallbackState as *mut c_void;

            // Register render callback.
            let render_cb = AURenderCallbackStruct {
                inputProc: Some(render_callback),
                inputProcRefCon: cb_ptr,
            };
            au_check!(
                AudioUnitSetProperty(
                    au_instance,
                    kAudioUnitProperty_SetRenderCallback,
                    Scope::Input as u32,
                    Element::Output as u32,
                    &render_cb as *const AURenderCallbackStruct as *const c_void,
                    std::mem::size_of::<AURenderCallbackStruct>() as u32,
                ),
                "SetRenderCallback"
            );

            // Uninitialize (was initialized by AudioUnit::new), then re-initialize
            // after format changes.
            au_check!(AudioUnitUninitialize(au_instance), "AudioUnitUninitialize");
            au_check!(AudioUnitInitialize(au_instance), "AudioUnitInitialize");

            // Start IO.
            au_check!(AudioOutputUnitStart(au_instance), "AudioOutputUnitStart");

            let info = SinkInfo {
                name: DEVICE_NAME.to_string(),
                uid: format!("coreaudio-device-{device_id}"),
                sample_rate: 48_000,
                channels: 2,
            };

            Ok(SystemBlackHoleSink {
                au_instance,
                callback_state: Some(callback_state),
                ring,
                info,
                last_stereo_frame: Vec::with_capacity(2048),
            })
        }
    }

    /// Number of buffer underruns since the sink was opened.
    pub fn underrun_count(&self) -> u64 {
        if let Some(state) = &self.callback_state {
            state.underruns.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Gracefully shut down: clear the render callback, stop, uninitialize, and dispose.
    ///
    /// Calling `drop` implicitly calls this.  Safe to call multiple times.
    fn shutdown(&mut self) {
        if self.callback_state.is_none() {
            return; // already shut down
        }
        unsafe {
            // 1. Clear the render callback so the real-time thread stops calling into our state.
            let null_cb = AURenderCallbackStruct {
                inputProc: None,
                inputProcRefCon: std::ptr::null_mut(),
            };
            AudioUnitSetProperty(
                self.au_instance,
                kAudioUnitProperty_SetRenderCallback,
                Scope::Input as u32,
                Element::Output as u32,
                &null_cb as *const AURenderCallbackStruct as *const c_void,
                std::mem::size_of::<AURenderCallbackStruct>() as u32,
            );

            // 2. Brief sleep so any in-flight callback completes.
            std::thread::sleep(std::time::Duration::from_millis(20));

            // 3. Stop and uninitialize.
            AudioOutputUnitStop(self.au_instance);
            AudioUnitUninitialize(self.au_instance);
            AudioComponentInstanceDispose(self.au_instance);
        }

        // 4. Drop the callback state — safe because no callback can fire after step 1+2.
        self.callback_state = None;
    }
}

impl Drop for SystemBlackHoleSink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl AudioSink for SystemBlackHoleSink {
    fn write_mono_s16(&mut self, pcm: &[i16]) -> Result<(), SinkError> {
        // Expand mono → stereo interleaved into the persistent buffer.  This
        // buffer doubles as the source for the top-up pass below, avoiding a
        // re-allocation per call.
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

        // Top-up: if the ring has drained below the safe water mark, repeat
        // the just-pushed frame to prevent the CoreAudio render callback from
        // underrunning between server ticks.  Capped at 10 repetitions
        // (50 ms) to avoid runaway buffer growth.
        const TARGET_MS: u32 = 30;
        let target_samples: usize = 48 * TARGET_MS as usize * 2; // 48 samples/ms × 2 ch
        let mut topups = 0;
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
        // Best-effort: ignore ring-full errors during drain.
        self.ring.push(&silence);
        // Give the callback time to consume the silence before shutdown.
        std::thread::sleep(std::time::Duration::from_millis(duration_ms as u64 + 10));
    }

    fn underrun_count(&self) -> u64 {
        SystemBlackHoleSink::underrun_count(self)
    }
}
