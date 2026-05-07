// Windows-only implementation — compiled only when target_os = "windows"
// TODO T05+: implement WASAPI virtual device integration

#[cfg(not(target_os = "windows"))]
pub fn placeholder() {}

#[cfg(target_os = "windows")]
pub fn placeholder() {
    // TODO T05+: WASAPI implementation
}
