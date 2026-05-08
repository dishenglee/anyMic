//! Integration tests for the anyMic mDNS discovery module.

use anymic_core::discovery::{
    compute_fingerprint, DiscoveryError, DiscoveryResponder, MdnsResponder, ServiceConfig,
};

// ── Helper ────────────────────────────────────────────────────────────────────

fn make_cfg() -> ServiceConfig {
    let fp = compute_fingerprint("test-host:uid-1234");
    ServiceConfig::for_macos_default("test-host".into(), fp)
}

// ── 1. TXT fields ─────────────────────────────────────────────────────────────

#[test]
fn txt_records_contain_required_keys() {
    let cfg = make_cfg();
    let txt = cfg.to_txt_records();

    for key in ["v", "name", "os", "codec", "fid", "ctl"] {
        assert!(
            txt.contains_key(key),
            "TXT record missing required key '{key}'"
        );
    }

    assert_eq!(txt["v"], "1");
    assert_eq!(txt["os"], "mac");
    assert_eq!(txt["codec"], "opus48");
    assert_eq!(txt["ctl"], "50128");
}

// ── 2. Fingerprint stability ──────────────────────────────────────────────────

#[test]
fn fingerprint_is_stable() {
    let seed = "MacBook-Pro:cafebabe";
    let a = compute_fingerprint(seed);
    let b = compute_fingerprint(seed);
    assert_eq!(a, b, "fingerprint must be deterministic for the same seed");
}

// ── 3. Fingerprint format ─────────────────────────────────────────────────────

#[test]
fn fingerprint_length_and_charset() {
    let fp = compute_fingerprint("any-seed");
    assert_eq!(fp.len(), 8, "fingerprint must be exactly 8 chars");
    assert!(
        fp.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "fingerprint must be lowercase hex [0-9a-f], got: {fp}"
    );
}

// ── 4. start / stop idempotency ───────────────────────────────────────────────

#[test]
fn start_stop_idempotent() {
    let mut r = MdnsResponder::new().expect("daemon should start");

    assert!(!r.is_running());

    r.start(make_cfg()).expect("first start should succeed");
    assert!(r.is_running());

    r.stop();
    assert!(!r.is_running());

    // Second stop must be a no-op
    r.stop();
    assert!(!r.is_running());
}

// ── 5. Double-start semantics ─────────────────────────────────────────────────

#[test]
fn double_start_returns_err() {
    let mut r = MdnsResponder::new().expect("daemon should start");

    r.start(make_cfg()).expect("first start should succeed");

    let result = r.start(make_cfg());
    assert!(
        result.is_err(),
        "second start while running must return Err"
    );

    // Verify the error is the right kind
    match result.unwrap_err() {
        DiscoveryError::Daemon(_) => {} // expected
        e => panic!("expected DiscoveryError::Daemon, got {e}"),
    }

    r.stop();
}

// ── 6. Self-browse integration test ──────────────────────────────────────────
//
// Marked `#[ignore]` because:
//   a) Multicast requires a real network interface (unavailable in many CIs).
//   b) mDNS port 5353 may be in use or blocked.
//
// Run manually with:
//   cargo test -p anymic-core --test discovery_mdns -- --ignored

#[test]
#[ignore]
fn self_browse_finds_registered_service() {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    use std::time::{Duration, Instant};

    let mut responder = MdnsResponder::new().expect("responder daemon");
    let cfg = make_cfg();
    responder.start(cfg).expect("start");

    // Give the daemon a moment to announce before we start browsing.
    std::thread::sleep(Duration::from_millis(200));

    // Browse using an independent daemon instance.
    let browser = ServiceDaemon::new().expect("browser daemon");
    let rx = browser.browse(ServiceConfig::SERVICE_TYPE).expect("browse");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                // Check TXT contains v=1
                let props = info.get_properties();
                let has_v1 = props.iter().any(|p| p.key() == "v" && p.val_str() == "1");
                if has_v1 {
                    found = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    assert!(found, "expected to find our service with v=1 within 5 s");

    // Now stop the responder and wait for ServiceRemoved.
    responder.stop();
    std::thread::sleep(Duration::from_millis(200));

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut removed = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                if fullname.contains(ServiceConfig::SERVICE_TYPE.trim_end_matches('.')) {
                    removed = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    let _ = browser.shutdown();
    assert!(removed, "expected ServiceRemoved within 5 s after stop");
}
