// Linux-only implementation — compiled only when target_os = "linux"
// TODO T06+: implement PipeWire / PulseAudio virtual device integration

#[cfg(not(target_os = "linux"))]
pub fn placeholder() {}

#[cfg(target_os = "linux")]
pub fn placeholder() {
    // TODO T06+: PipeWire/PulseAudio implementation
}
