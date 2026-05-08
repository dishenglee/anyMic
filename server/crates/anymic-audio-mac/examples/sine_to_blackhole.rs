//! Writes 5 seconds of 1 kHz sine wave (48 kHz, mono s16, peak 0.3) into BlackHole 2ch.

use anymic_core::sink::AudioSink;

fn main() {
    let mut sink = anymic_audio_mac::SystemBlackHoleSink::open()
        .expect("BlackHole 2ch not found — make sure it's installed");

    let mut t = 0u32;
    let frame_samples = 240usize; // 5 ms @ 48k
    let total_frames = (5 * 48000) / frame_samples;

    for _ in 0..total_frames {
        let mut buf = [0i16; 240];
        for s in &mut buf {
            let v =
                (2.0 * std::f32::consts::PI * 1000.0 * (t as f32) / 48000.0).sin() * 0.3 * 32767.0;
            *s = v as i16;
            t = t.wrapping_add(1);
        }
        sink.write_mono_s16(&buf).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    sink.drain_silence(50);
    println!("done");
}
