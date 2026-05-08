// macOS audio device integration for anyMic (CoreAudio / BlackHole)

#[cfg(target_os = "macos")]
mod blackhole;

#[cfg(target_os = "macos")]
pub use blackhole::SystemBlackHoleSink;
