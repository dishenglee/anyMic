//! Manual verification demo for mDNS service announcement.
//!
//! Registers the anyMic service and keeps it advertised for 30 seconds, then
//! gracefully unregisters (sends a mDNS goodbye).
//!
//! While this is running, you can verify discovery with:
//!
//! ```shell
//! dns-sd -B _anymic._udp local.
//! # or
//! avahi-browse _anymic._udp
//! ```
//!
//! Run with:
//!
//! ```shell
//! cargo run -p anymic-core --example announce_demo
//! ```

use anymic_core::discovery::{
    compute_fingerprint, DiscoveryResponder, MdnsResponder, ServiceConfig,
};
use std::time::Duration;

fn main() {
    // Derive a stable fingerprint from the machine hostname + a fixed suffix.
    let host = hostname();
    let fingerprint = compute_fingerprint(&format!("{host}:anymic-demo"));

    let cfg = ServiceConfig::for_macos_default(host.clone(), fingerprint.clone());

    println!("anyMic announce_demo");
    println!("  host        : {host}");
    println!("  instance    : {}", cfg.instance_name);
    println!("  fingerprint : {fingerprint}");
    println!("  data port   : {}", cfg.data_port);
    println!("  control port: {}", cfg.control_port);
    println!("  service type: {}", ServiceConfig::SERVICE_TYPE);
    println!();
    println!("Registering service … (run `dns-sd -B _anymic._udp local.` in another terminal)");
    println!();

    let mut responder = MdnsResponder::new().expect("failed to create mDNS daemon");
    responder.start(cfg).expect("failed to register service");

    println!("Service registered. Advertising for 30 seconds …");
    std::thread::sleep(Duration::from_secs(30));

    println!("Stopping — sending mDNS goodbye …");
    responder.stop();
    // Give the goodbye packet time to propagate before we exit.
    std::thread::sleep(Duration::from_millis(500));
    println!("Done.");
}

/// Return the local machine hostname (falls back to "localhost" on error).
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}
