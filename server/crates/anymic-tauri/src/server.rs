use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

#[cfg(target_os = "macos")]
use anymic_audio_mac::SystemBlackHoleSink as PlatformSink;

#[cfg(target_os = "windows")]
use anymic_audio_win::SystemVbCableSink as PlatformSink;

use anymic_core::decoder::{FrameDecoder, OpusFrameDecoder};
use anymic_core::discovery::{
    compute_fingerprint, DiscoveryResponder, MdnsResponder, ServiceConfig,
};
use anymic_core::jitter::{JitterBuffer, JitterOut};
use anymic_core::packet::RtpPacket;
use anymic_core::sink::AudioSink;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::state::SharedStats;
use crate::stats::{LiveJitterStats, LiveStats};

// ── errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("failed to bind UDP port {port}: {source}")]
    BindUdp { port: u16, source: std::io::Error },
    #[error("failed to bind TCP port {port}: {source}")]
    BindTcp { port: u16, source: std::io::Error },
    #[error("virtual audio device not found — install BlackHole (macOS): https://existential.audio/blackhole/ or VB-CABLE (Windows): https://vb-audio.com/Cable/")]
    BlackHoleNotFound,
    #[error("audio sink error: {0}")]
    Sink(String),
    #[error("decoder init failed: {0}")]
    Decoder(String),
    #[error("mDNS error: {0}")]
    Mdns(String),
}

// ── config ────────────────────────────────────────────────────────────────────

pub struct ServerConfig {
    /// UDP data port (default 50127).
    pub data_port: u16,
    /// TCP control port (default 50128).
    pub control_port: u16,
    /// mDNS instance name (default "anyMic-<hostname>").
    pub instance_name: String,
    /// Samples per audio frame (default 240 = 5 ms @ 48 kHz).
    pub frame_samples: u32,
    /// How many consecutive missing frames before Disconnected (default 500 ≈ 2.5 s).
    pub max_stall_frames: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let hostname = hostname::get()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let instance = if hostname.is_empty() {
            "anyMic".to_string()
        } else {
            format!("anyMic-{hostname}")
        };
        Self {
            data_port: 50127,
            control_port: 50128,
            instance_name: instance,
            frame_samples: 240,
            max_stall_frames: 500,
        }
    }
}

// ── system mDNS bridge ────────────────────────────────────────────────────────

/// RAII guard that kills the spawned `dns-sd -R` child process on drop.
///
/// We register anyMic with the system mDNSResponder via the `dns-sd` CLI so
/// that standard mDNS clients (Android NsdManager, iOS, `dns-sd` itself, any
/// app going through the system daemon) can discover us.  The pure-Rust
/// `mdns-sd` library runs on its own multicast socket and is invisible to
/// anything that queries through the system daemon, which is the majority
/// of real clients.
struct DnsSdGuard(Option<std::process::Child>);

impl Drop for DnsSdGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(target_os = "macos")]
fn spawn_dns_sd_register(
    instance_name: &str,
    data_port: u16,
    control_port: u16,
    host_name: &str,
    fingerprint: &str,
) -> Option<std::process::Child> {
    use std::process::{Command, Stdio};
    let args = [
        "-R".to_string(),
        instance_name.to_string(),
        "_anymic._udp".to_string(),
        "local".to_string(),
        data_port.to_string(),
        "v=1".to_string(),
        format!("name={host_name}"),
        "os=mac".to_string(),
        "codec=opus48".to_string(),
        format!("fid={fingerprint}"),
        format!("ctl={control_port}"),
    ];
    Command::new("/usr/bin/dns-sd")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(not(target_os = "macos"))]
fn spawn_dns_sd_register(
    _instance_name: &str,
    _data_port: u16,
    _control_port: u16,
    _host_name: &str,
    _fingerprint: &str,
) -> Option<std::process::Child> {
    None
}

// ── local IP detection ────────────────────────────────────────────────────────

/// Best-effort detection of the LAN IPv4 address that other devices on the
/// same network can reach.
///
/// Strategy:
///   1. (macOS) Probe primary network interfaces via `ipconfig getifaddr`.
///      en0 is the main Wi-Fi / Ethernet on most Apple Silicon Macs;
///      en1–en4 cover Thunderbolt and USB-C network adapters.
///   2. Fall back to the "connect 8.8.8.8" trick, but reject non-RFC1918
///      ranges (198.18.0.0/15 used by ClashX/Surge proxies, link-local,
///      loopback).
fn detect_local_ip() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        for iface in &["en0", "en1", "en2", "en3", "en4"] {
            if let Some(ip) = ipconfig_getifaddr(iface) {
                if is_usable_lan_ip(&ip) {
                    return Some(ip);
                }
            }
        }
    }

    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip_str = sock.local_addr().ok()?.ip().to_string();
    if is_usable_lan_ip(&ip_str) {
        Some(ip_str)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn ipconfig_getifaddr(iface: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/ipconfig")
        .args(["getifaddr", iface])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Accept only RFC 1918 private IPv4 ranges; reject benchmarking / link-local /
/// loopback / public addresses (which would mean a VPN/proxy interface picked
/// up the route).
fn is_usable_lan_ip(ip: &str) -> bool {
    let parts: Vec<u8> = match ip
        .split('.')
        .map(|p| p.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) if v.len() == 4 => v,
        _ => return false,
    };
    let (a, b) = (parts[0], parts[1]);
    // 10.0.0.0/8
    if a == 10 { return true; }
    // 172.16.0.0/12
    if a == 172 && (16..32).contains(&b) { return true; }
    // 192.168.0.0/16
    if a == 192 && b == 168 { return true; }
    false
}

// ── owned packet (bridges UDP → tick task without lifetime) ───────────────────

struct OwnedPacket {
    seq: u16,
    timestamp: u32,
    ssrc16: u16,
    payload: Vec<u8>,
}

impl OwnedPacket {
    fn from_rtp(pkt: &RtpPacket<'_>, _src: SocketAddr) -> Self {
        OwnedPacket {
            seq: pkt.seq,
            timestamp: pkt.timestamp,
            ssrc16: pkt.ssrc16,
            payload: pkt.payload.to_vec(),
        }
    }
}

// ── server handle ─────────────────────────────────────────────────────────────

pub struct ServerHandle {
    shutdown_tx: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
    stats: SharedStats,
}

impl ServerHandle {
    /// Gracefully stop the server and wait for all tasks to finish.
    pub async fn stop(self) -> anyhow::Result<()> {
        let _ = self.shutdown_tx.send(true);
        self.join.await?;
        Ok(())
    }

    /// Access the shared stats.
    pub fn stats(&self) -> SharedStats {
        Arc::clone(&self.stats)
    }
}

// ── start_server ──────────────────────────────────────────────────────────────

pub async fn start_server(cfg: ServerConfig) -> Result<ServerHandle, ServerError> {
    // Bind sockets eagerly on the calling task so we surface port errors immediately.
    let udp_sock = UdpSocket::bind(("0.0.0.0", cfg.data_port))
        .await
        .map_err(|e| ServerError::BindUdp {
            port: cfg.data_port,
            source: e,
        })?;
    let actual_data_port = udp_sock
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(cfg.data_port);

    let tcp_listener = TcpListener::bind(("0.0.0.0", cfg.control_port))
        .await
        .map_err(|e| ServerError::BindTcp {
            port: cfg.control_port,
            source: e,
        })?;
    let actual_control_port = tcp_listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(cfg.control_port);

    // Open platform sink (sync, but fast).
    let sink: Box<dyn AudioSink> = Box::new(PlatformSink::open().map_err(|e| {
        if e.to_string().contains("not found") || e.to_string().contains("DeviceNotFound") {
            ServerError::BlackHoleNotFound
        } else {
            ServerError::Sink(e.to_string())
        }
    })?);
    let sink_name = sink.info().name.clone();

    // Decoder.
    let decoder = OpusFrameDecoder::new_voip().map_err(|e| ServerError::Decoder(e.to_string()))?;

    // mDNS.
    let hostname = hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let fingerprint = compute_fingerprint(&hostname);
    let mut mdns = MdnsResponder::new().map_err(|e| ServerError::Mdns(e.to_string()))?;
    #[cfg(target_os = "macos")]
    let os_label = "mac";
    #[cfg(target_os = "windows")]
    let os_label = "win";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let os_label = "other";

    let mdns_cfg = ServiceConfig {
        instance_name: cfg.instance_name.clone(),
        data_port: actual_data_port,
        control_port: actual_control_port,
        host_name: hostname.clone(),
        os: os_label,
        codec: "opus48",
        fingerprint: fingerprint.clone(),
        extra_txt: Default::default(),
    };
    let mdns_ok = mdns.start(mdns_cfg).is_ok();

    // Register with the system mDNSResponder so standard clients (Android
    // NsdManager, iOS, dns-sd CLI) can discover us.  The pure-Rust mdns-sd
    // library is invisible to anyone going through the system daemon.
    let dnssd_child = spawn_dns_sd_register(
        &cfg.instance_name,
        actual_data_port,
        actual_control_port,
        &hostname,
        &fingerprint,
    );
    if dnssd_child.is_some() {
        info!("system mDNSResponder registration via dns-sd OK");
    } else {
        warn!("system mDNSResponder registration unavailable (dns-sd missing?)");
    }

    info!(
        data_port = actual_data_port,
        control_port = actual_control_port,
        mdns = mdns_ok,
        sink = %sink_name,
        "server started"
    );

    // Shared stats.
    let stats: SharedStats = Arc::new(Mutex::new(LiveStats {
        running: true,
        bound_data_port: actual_data_port,
        bound_control_port: actual_control_port,
        mdns_registered: mdns_ok,
        sink_name: Some(sink_name),
        local_ip: detect_local_ip(),
        ..Default::default()
    }));

    // Shutdown channel.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Packet channel from UDP recv to tick task.
    let (pkt_tx, pkt_rx) = mpsc::channel::<OwnedPacket>(512);

    // Spawn all tasks under a single JoinHandle.
    let stats_clone = Arc::clone(&stats);
    let shutdown_rx2 = shutdown_rx.clone();
    let shutdown_rx3 = shutdown_rx.clone();

    let join = tokio::spawn(async move {
        let udp_task = tokio::spawn(udp_recv_task(
            udp_sock,
            pkt_tx,
            Arc::clone(&stats_clone),
            shutdown_rx,
        ));

        let tick_task = tokio::spawn(tick_task(
            pkt_rx,
            sink,
            Box::new(decoder),
            cfg.frame_samples,
            cfg.max_stall_frames,
            Arc::clone(&stats_clone),
            shutdown_rx2,
        ));

        let ctrl_task = tokio::spawn(control_task(tcp_listener, shutdown_rx3));

        // Drop guards inside here so the registrations live until shutdown.
        let _mdns_guard = mdns;
        let _dnssd_guard = DnsSdGuard(dnssd_child);
        let _ = tokio::join!(udp_task, tick_task, ctrl_task);

        {
            let mut s = stats_clone.lock();
            s.running = false;
            s.mdns_registered = false;
        }
        info!("server shutdown complete");
    });

    Ok(ServerHandle {
        shutdown_tx,
        join,
        stats,
    })
}

// ── Task A: UDP recv ──────────────────────────────────────────────────────────

async fn udp_recv_task(
    sock: UdpSocket,
    pkt_tx: mpsc::Sender<OwnedPacket>,
    stats: SharedStats,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut buf = [0u8; 1500];
    let start = Instant::now();

    loop {
        let (n, src) = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
                continue;
            }
            result = sock.recv_from(&mut buf) => {
                match result {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("UDP recv error: {e}");
                        continue;
                    }
                }
            }
        };

        let data = &buf[..n];
        let elapsed_ms = start.elapsed().as_millis() as u64;

        match RtpPacket::decode(data) {
            Ok(pkt) => {
                let owned = OwnedPacket::from_rtp(&pkt, src);
                let mut s = stats.lock();
                s.udp_packets_received += 1;
                s.udp_bytes_received += n as u64;
                s.last_client_addr = Some(src.to_string());
                s.last_packet_at_ms_since_start = Some(elapsed_ms);
                drop(s);

                if pkt_tx.try_send(owned).is_err() {
                    debug!("packet channel full; dropping packet");
                }
            }
            Err(e) => {
                let mut s = stats.lock();
                s.udp_decode_errors += 1;
                drop(s);
                debug!("RTP decode error from {src}: {e}");
            }
        }
    }

    info!("UDP recv task exiting");
}

// ── Task B: 5 ms tick ─────────────────────────────────────────────────────────

async fn tick_task(
    mut pkt_rx: mpsc::Receiver<OwnedPacket>,
    mut sink: Box<dyn AudioSink>,
    mut decoder: Box<dyn FrameDecoder>,
    frame_samples: u32,
    max_stall_frames: u32,
    stats: SharedStats,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    use anymic_core::packet::{Flags, PayloadType};
    use std::borrow::Cow;

    let mut jitter = JitterBuffer::new(frame_samples, max_stall_frames);
    let mut now_samples: u32 = 0;

    // Cross-fade state: when the previous frame was synthesised (PLC or Stall),
    // the first CROSSFADE_SAMPLES samples of the next real frame are linearly
    // mixed with the saved tail of the synthesised frame to mask the phase
    // discontinuity that produces an audible click at the boundary.
    const CROSSFADE_SAMPLES: usize = 96; // 2 ms @ 48 kHz
    let mut prev_was_synthesized = false;
    let mut last_pcm_tail: [i16; CROSSFADE_SAMPLES] = [0; CROSSFADE_SAMPLES];

    // Periodic stats log every 5 s for diagnosing audio glitches.
    let mut last_stats_log = std::time::Instant::now();
    let stats_log_interval = std::time::Duration::from_secs(5);
    let server_start = std::time::Instant::now();

    let tick_dur = tokio::time::Duration::from_millis(5);
    let mut interval = tokio::time::interval(tick_dur);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = interval.tick() => {
                // Drain all pending packets from channel into jitter buffer.
                loop {
                    match pkt_rx.try_recv() {
                        Ok(owned) => {
                            // Reconstruct a static RtpPacket from owned data.
                            let rtp = RtpPacket {
                                version_minor: 0,
                                flags: Flags::default(),
                                payload_type: PayloadType::Opus48kMono,
                                seq: owned.seq,
                                timestamp: owned.timestamp,
                                ssrc16: owned.ssrc16,
                                payload: Cow::Owned(owned.payload),
                            };
                            jitter.push(rtp);
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }

                // Pop one frame from the jitter buffer.
                let out = jitter.pop_due(now_samples);
                now_samples = now_samples.wrapping_add(frame_samples);

                match out {
                    JitterOut::Frame { payload, .. } => {
                        match decoder.decode(&payload) {
                            Ok(mut pcm) => {
                                // Cross-fade after a synthesised frame to mask
                                // PLC↔real boundary clicks.
                                if prev_was_synthesized && pcm.len() >= CROSSFADE_SAMPLES {
                                    for i in 0..CROSSFADE_SAMPLES {
                                        let alpha =
                                            (i + 1) as f32 / (CROSSFADE_SAMPLES + 1) as f32;
                                        let prev = last_pcm_tail[i] as f32;
                                        let new_s = pcm[i] as f32;
                                        pcm[i] = ((1.0 - alpha) * prev + alpha * new_s) as i16;
                                    }
                                }
                                if pcm.len() >= CROSSFADE_SAMPLES {
                                    last_pcm_tail.copy_from_slice(
                                        &pcm[pcm.len() - CROSSFADE_SAMPLES..],
                                    );
                                }
                                prev_was_synthesized = false;

                                if let Err(e) = sink.write_mono_s16(&pcm) {
                                    warn!("sink write error: {e}");
                                } else {
                                    let mut s = stats.lock();
                                    s.frames_written_to_sink += 1;
                                    s.jitter = Some(LiveJitterStats::from(jitter.stats()));
                                }
                            }
                            Err(e) => {
                                warn!("opus decode error: {e}");
                                let mut s = stats.lock();
                                s.udp_decode_errors += 1;
                            }
                        }
                    }
                    JitterOut::Plc { samples } => {
                        let pcm = decoder.decode_plc(samples as usize);
                        if pcm.len() >= CROSSFADE_SAMPLES {
                            last_pcm_tail
                                .copy_from_slice(&pcm[pcm.len() - CROSSFADE_SAMPLES..]);
                        }
                        prev_was_synthesized = true;
                        let _ = sink.write_mono_s16(&pcm);
                        let mut s = stats.lock();
                        s.plc_emitted += 1;
                        s.jitter = Some(LiveJitterStats::from(jitter.stats()));
                    }
                    JitterOut::Stall => {
                        // Use Opus PLC instead of writing zero samples to avoid the
                        // audible click at the audio↔silence boundary.  The PLC
                        // path naturally decays to silence over ~100 ms without a
                        // discontinuity.
                        let pcm = decoder.decode_plc(frame_samples as usize);
                        if pcm.len() >= CROSSFADE_SAMPLES {
                            last_pcm_tail
                                .copy_from_slice(&pcm[pcm.len() - CROSSFADE_SAMPLES..]);
                        }
                        prev_was_synthesized = true;
                        let _ = sink.write_mono_s16(&pcm);
                        let mut s = stats.lock();
                        s.stall_events += 1;
                        s.jitter = Some(LiveJitterStats::from(jitter.stats()));
                    }
                    JitterOut::Disconnected => {
                        info!("jitter buffer: Disconnected — resetting session");
                        jitter.reset();
                        decoder.reset();
                        prev_was_synthesized = false;
                        // Drain silence to clear the ring buffer.
                        sink.drain_silence(200);
                        let mut s = stats.lock();
                        s.jitter = Some(LiveJitterStats::from(jitter.stats()));
                    }
                }

                stats.lock().server_uptime_ms = server_start.elapsed().as_millis() as u64;

                if last_stats_log.elapsed() >= stats_log_interval {
                    let underruns = sink.underrun_count();
                    let s = stats.lock();
                    let p95 = s.jitter.as_ref().map(|j| j.jitter_ms_p95).unwrap_or(0.0);
                    let depth = s.jitter.as_ref().map(|j| j.target_depth).unwrap_or(0);
                    info!(
                        udp_pkts = s.udp_packets_received,
                        frames = s.frames_written_to_sink,
                        plc = s.plc_emitted,
                        stall = s.stall_events,
                        jitter_p95_ms = p95,
                        target_depth = depth,
                        sink_underruns = underruns,
                        "tick stats"
                    );
                    drop(s);
                    last_stats_log = std::time::Instant::now();
                }
            }
        }
    }

    info!("tick task exiting");
}

// ── Task C: TCP control ───────────────────────────────────────────────────────

async fn control_task(listener: TcpListener, mut shutdown_rx: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            result = listener.accept() => {
                match result {
                    Ok((mut stream, addr)) => {
                        info!("TCP control connection from {addr}");
                        tokio::spawn(async move {
                            use tokio::io::AsyncWriteExt;
                            // MVP: send a simple ACK and close.
                            let _ = stream.write_all(b"OK\n").await;
                            debug!("ACK sent to {addr}");
                        });
                    }
                    Err(e) => {
                        warn!("TCP accept error: {e}");
                    }
                }
            }
        }
    }

    info!("control task exiting");
}
