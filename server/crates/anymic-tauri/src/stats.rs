use anymic_core::jitter::JitterStats;
use serde::Serialize;

/// Serializable snapshot of jitter buffer stats.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveJitterStats {
    pub received: u64,
    pub late_dropped: u64,
    pub duplicates: u64,
    pub plc_emitted: u64,
    pub stall_events: u64,
    pub current_depth: u8,
    pub target_depth: u8,
    pub jitter_ms_p95: f32,
}

impl From<JitterStats> for LiveJitterStats {
    fn from(s: JitterStats) -> Self {
        Self {
            received: s.received,
            late_dropped: s.late_dropped,
            duplicates: s.duplicates,
            plc_emitted: s.plc_emitted,
            stall_events: s.stall_events,
            current_depth: s.current_depth,
            target_depth: s.target_depth,
            jitter_ms_p95: s.jitter_ms_p95,
        }
    }
}

/// Live statistics snapshot exposed to the Tauri frontend.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveStats {
    pub running: bool,
    pub bound_data_port: u16,
    pub bound_control_port: u16,
    pub mdns_registered: bool,
    pub udp_packets_received: u64,
    pub udp_bytes_received: u64,
    pub udp_decode_errors: u64,
    pub frames_written_to_sink: u64,
    pub plc_emitted: u64,
    pub stall_events: u64,
    pub jitter: Option<LiveJitterStats>,
    pub sink_name: Option<String>,
    pub last_client_addr: Option<String>,
    pub last_packet_at_ms_since_start: Option<u64>,
    /// Local LAN IPv4 address — what users enter on the phone for manual connect.
    pub local_ip: Option<String>,
    /// Milliseconds since the server started.  The UI uses this together with
    /// `last_packet_at_ms_since_start` to compute "client active"
    /// (last packet < 3 s ago).
    pub server_uptime_ms: u64,
}
