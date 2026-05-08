//! mDNS / Bonjour service responder for anyMic.
//!
//! Registers the desktop server as `_anymic._udp.local.` so that Android
//! clients can discover it with a standard DNS-SD browse.

use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use thiserror::Error;

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mdns daemon error: {0}")]
    Daemon(String),
    #[error("invalid service config: {0}")]
    InvalidConfig(String),
}

// ── ServiceConfig ─────────────────────────────────────────────────────────────

/// All parameters required to advertise an anyMic server over mDNS.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Instance name shown in Bonjour browsers (e.g. "MacBook" or "anyMic-MacBook").
    pub instance_name: String,
    /// UDP audio-data port (protocol default 50127).
    pub data_port: u16,
    /// TCP control port carried in TXT `ctl=` (protocol default 50128).
    pub control_port: u16,
    /// Human-readable hostname (e.g. "MacBook").
    pub host_name: String,
    /// Operating-system tag: `"mac"`, `"win"`, or `"linux"`.
    pub os: &'static str,
    /// Codec string: `"opus48"`.
    pub codec: &'static str,
    /// 8-char lowercase hex fingerprint (see [`compute_fingerprint`]).
    pub fingerprint: String,
    /// Optional extra TXT key/value pairs beyond the required six.
    pub extra_txt: HashMap<String, String>,
}

impl ServiceConfig {
    /// DNS-SD service type advertised on the LAN.
    pub const SERVICE_TYPE: &'static str = "_anymic._udp.local.";

    /// Convenience constructor with sensible macOS defaults.
    ///
    /// * `data_port`    → 50127
    /// * `control_port` → 50128
    /// * `os`           → `"mac"`
    /// * `codec`        → `"opus48"`
    /// * `instance_name`→ `"anyMic-<host_name>"`
    pub fn for_macos_default(host_name: String, fingerprint: String) -> Self {
        Self {
            instance_name: format!("anyMic-{}", host_name),
            data_port: 50127,
            control_port: 50128,
            host_name,
            os: "mac",
            codec: "opus48",
            fingerprint,
            extra_txt: HashMap::new(),
        }
    }

    /// Build the full TXT record map.
    ///
    /// Required keys (protocol §4.3): `v`, `name`, `os`, `codec`, `fid`, `ctl`.
    /// Any entries in `self.extra_txt` are merged in afterwards.
    pub fn to_txt_records(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("v".to_string(), "1".to_string());
        map.insert("name".to_string(), self.host_name.clone());
        map.insert("os".to_string(), self.os.to_string());
        map.insert("codec".to_string(), self.codec.to_string());
        map.insert("fid".to_string(), self.fingerprint.clone());
        map.insert("ctl".to_string(), self.control_port.to_string());
        // Merge extra fields (they may not shadow required keys, but caller's choice).
        for (k, v) in &self.extra_txt {
            map.entry(k.clone()).or_insert_with(|| v.clone());
        }
        map
    }

    /// Validate the config before attempting registration.
    fn validate(&self) -> Result<(), DiscoveryError> {
        if self.instance_name.is_empty() {
            return Err(DiscoveryError::InvalidConfig(
                "instance_name must not be empty".into(),
            ));
        }
        if self.fingerprint.len() != 8 {
            return Err(DiscoveryError::InvalidConfig(format!(
                "fingerprint must be 8 hex chars, got {} chars",
                self.fingerprint.len()
            )));
        }
        if !self.fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(DiscoveryError::InvalidConfig(
                "fingerprint must be lowercase hex [0-9a-f]".into(),
            ));
        }
        Ok(())
    }
}

// ── Fingerprint ───────────────────────────────────────────────────────────────

/// Compute an 8-char lowercase hex fingerprint from `seed`.
///
/// **Hash choice:** We use [`std::collections::hash_map::DefaultHasher`] with a
/// fixed seed (0) rather than pulling in an external SHA-1 or SHA-256 crate.
/// Rationale:
/// - The fingerprint's only job is to **disambiguate** two hosts that share the
///   same hostname (protocol §4.3 "fid").  Cryptographic strength is not needed.
/// - `DefaultHasher` is in `std`, adding zero dependencies and zero compile-time
///   overhead.
/// - 64-bit output → 16 hex chars → we take the first 8, giving 32 bits of
///   collision resistance — far more than enough for ≤ tens of hosts on a LAN.
///
/// The result is **stable within a single process / Rust release**, which is all
/// we require: the fingerprint is only compared against simultaneous discoveries.
/// If the Rust release changes `DefaultHasher`, the new fingerprint will be
/// different but still valid; the client simply shows a fresh entry.
pub fn compute_fingerprint(seed: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    let v = h.finish();
    // Take the low 32 bits → 8 hex chars.
    format!("{:08x}", v as u32)
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Interface for mDNS responders.  Implementations must be `Send` so they can
/// be moved into background threads.
pub trait DiscoveryResponder: Send {
    /// Register the service described by `cfg` on the local network.
    ///
    /// Returns `Err` if already running or if `cfg` is invalid.
    fn start(&mut self, cfg: ServiceConfig) -> Result<(), DiscoveryError>;

    /// Unregister the service and send a mDNS goodbye packet.
    /// Idempotent: calling `stop` when already stopped is a no-op.
    fn stop(&mut self);

    /// Returns `true` iff the service is currently registered.
    fn is_running(&self) -> bool;
}

// ── MdnsResponder ─────────────────────────────────────────────────────────────

/// Production mDNS responder backed by the `mdns-sd` crate.
///
/// A single `ServiceDaemon` is created on `MdnsResponder::new` and kept alive
/// for the responder's lifetime.  Calling `start` registers one service;
/// `stop` unregisters it.  The daemon is shut down when the `MdnsResponder` is
/// dropped.
pub struct MdnsResponder {
    daemon: ServiceDaemon,
    fullname: Option<String>, // None when not running
}

impl MdnsResponder {
    /// Create a new responder.  Spawns the `mdns-sd` background thread.
    pub fn new() -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Daemon(e.to_string()))?;
        Ok(Self {
            daemon,
            fullname: None,
        })
    }
}

impl DiscoveryResponder for MdnsResponder {
    fn start(&mut self, cfg: ServiceConfig) -> Result<(), DiscoveryError> {
        if self.fullname.is_some() {
            return Err(DiscoveryError::Daemon(
                "responder is already running; call stop() first".into(),
            ));
        }

        cfg.validate()?;

        let txt = cfg.to_txt_records();

        // Build a host label for mDNS: must end with ".local."
        let host_label = format!("{}.local.", cfg.host_name);

        // Convert our HashMap to a slice-of-tuple so ServiceInfo::new accepts it.
        let txt_pairs: Vec<(&str, &str)> =
            txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        let service = ServiceInfo::new(
            ServiceConfig::SERVICE_TYPE,
            &cfg.instance_name,
            &host_label,
            "", // empty → addr_auto will fill in real interfaces
            cfg.data_port,
            txt_pairs.as_slice(),
        )
        .map_err(|e| DiscoveryError::InvalidConfig(e.to_string()))?
        .enable_addr_auto();

        let fullname = service.get_fullname().to_string();

        self.daemon
            .register(service)
            .map_err(|e| DiscoveryError::Daemon(e.to_string()))?;

        self.fullname = Some(fullname);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(ref name) = self.fullname.take() {
            // unregister returns a receiver; we intentionally ignore the result
            // so that stop() is always a no-op on failure.
            let _ = self.daemon.unregister(name);
        }
    }

    fn is_running(&self) -> bool {
        self.fullname.is_some()
    }
}

impl Drop for MdnsResponder {
    fn drop(&mut self) {
        self.stop();
        // Best-effort shutdown of the background thread.
        let _ = self.daemon.shutdown();
    }
}
