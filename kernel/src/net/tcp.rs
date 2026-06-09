//! TCP — Transmission Control Protocol
//!
//! nopeekOS-optimized defaults:
//! - No Nagle (low latency for request/response)
//! - 40ms delayed ACK (not 200ms)
//! - Initial window: 10 segments
//! - 3 retries, max 10s timeout (fast failure)
//! - Capability-gated: no cap = no connection

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::{Mutex, Once};
use super::{ipv4, arp};

// RFC 6528 — Initial Sequence Number generation. Predictable ISNs (e.g. a
// raw tick counter) let an off-path attacker forge in-window segments on a
// listening socket. We mix a per-boot CSPRNG secret with the connection
// 4-tuple via BLAKE3-keyed-hash, then add a tick-derived monotonic counter
// so retried connections still grow forward.
static ISN_SECRET: Once<[u8; 32]> = Once::new();

fn isn_secret() -> &'static [u8; 32] {
    ISN_SECRET.call_once(|| crate::csprng::random_256())
}

fn generate_isn(saddr: [u8; 4], daddr: [u8; 4], sport: u16, dport: u16) -> u32 {
    let mut buf = [0u8; 12];
    buf[0..4].copy_from_slice(&saddr);
    buf[4..8].copy_from_slice(&daddr);
    buf[8..10].copy_from_slice(&sport.to_be_bytes());
    buf[10..12].copy_from_slice(&dport.to_be_bytes());
    let h = blake3::keyed_hash(isn_secret(), &buf);
    let b = h.as_bytes();
    let hash_part = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    // Monotonic component: 100 Hz tick × 2500 ≈ 4 µs ISN step (RFC 6528 §3
    // suggests a ~250 kHz clock). Wrap is fine — the secret-keyed hash
    // ensures the absolute value is unguessable per 4-tuple.
    let timer = (crate::interrupts::ticks() as u32).wrapping_mul(2500);
    hash_part.wrapping_add(timer)
}

// A single LibreWolf page load opens ~20+ parallel TLS connections
// (CDNs, telemetry, OCSP, …). 16 was a single-`https`-intent ceiling;
// the browser exhausts it instantly → connects fail / stall →
// PR_IO_TIMEOUT_ERROR. 128 matches the NAT session table.
const MAX_CONNECTIONS: usize = 128;
const MSS: u16 = 1460; // standard Ethernet MSS
// The SYN/SYN-ACK window is never scaled (RFC 7323), so it's capped at 16-bit.
const INITIAL_WINDOW: u16 = 65535;
// TCP Window Scaling (RFC 7323). Without it the window is capped at 64 KiB and
// throughput = 64 KiB / RTT (~4 MB/s on a CDN regardless of link/NIC — the
// observed global slowness). We advertise `free >> OUR_WSCALE`.
// WSCALE 7 so the 16-bit window field can express the full 4 MiB buffer
// (4 MiB >> 7 = 32768 ≤ 65535). At WSCALE 5 a 1 MiB window capped a single
// flow at ~727 Mbit (1 MiB / ~11 ms RTT) — measured 585/710 Mbit vs 857 native;
// the receive window was the bottleneck, not the link.
const OUR_WSCALE: u8 = 7;

/// Current receive window for the TCP window field. Scaled by OUR_WSCALE once
/// window scaling has been negotiated, else the raw free space (≤ 64 KiB).
fn recv_window(conn: &TcpConn) -> u16 {
    let free = RECV_BUF_SIZE.saturating_sub(conn.recv_buf.len());
    if conn.wscale_ok {
        (free >> OUR_WSCALE).min(65535) as u16
    } else {
        free.min(65535) as u16
    }
}
const MAX_RETRIES: u8 = 3;
const RETRY_TICKS_BASE: u64 = 100; // 1 second (100Hz)
// 4 MiB receive buffer → ~4 MiB window with scaling → fills the bandwidth-delay
// product for ~gigabit even at tens-of-ms RTT (1 MiB was the cap at ~11 ms;
// higher-RTT CDNs need more). Grown lazily (VecDeque::new), so an idle
// connection costs nothing and only an actively-bursting one approaches 4 MiB.
// Host TCP only ever has a handful of live connections (OTA/https/dns), so the
// worst-case footprint is small; the guest browser uses its own (microvm) TCP.
const RECV_BUF_SIZE: usize = 4 * 1024 * 1024;
const DELAYED_ACK_TICKS: u64 = 4; // 40ms at 100Hz

// TCP flags
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

const HEADER_LEN: usize = 20; // no options (options added separately for SYN)

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum State {
    Closed,
    Listen,
    SynReceived,
    SynSent,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
    TimeWait,
}

#[allow(dead_code)]
struct TcpConn {
    state: State,
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,

    // Sequence numbers
    snd_nxt: u32, // next byte to send
    snd_una: u32, // oldest unacknowledged
    snd_iss: u32, // initial send seq
    rcv_nxt: u32, // next expected from remote
    rcv_irs: u32, // initial recv seq

    // Buffers
    recv_buf: VecDeque<u8>,
    send_buf: Vec<u8>,

    // Retransmit
    retries: u8,
    last_send_tick: u64,

    // Delayed ACK
    ack_pending: bool,
    ack_tick: u64,

    // Connection complete flag
    established: bool,
    closed: bool,
    error: bool,

    // Window scaling (RFC 7323). `wscale_ok` once both SYNs carried the
    // option; `snd_wscale` is the peer's shift (to scale their advertised
    // window). Our own advertised window is scaled by OUR_WSCALE.
    wscale_ok: bool,
    snd_wscale: u8,
}

static CONNECTIONS: Mutex<[Option<TcpConn>; MAX_CONNECTIONS]> = Mutex::new(
    [const { None }; MAX_CONNECTIONS]
);

static NEXT_PORT: Mutex<u16> = Mutex::new(49152);

fn alloc_port() -> u16 {
    let mut port = NEXT_PORT.lock();
    let p = *port;
    *port = if *port >= 65534 { 49152 } else { *port + 1 };
    p
}

/// Open a TCP connection. Returns connection handle (index). Blocking until established.
pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {
    let local_port = alloc_port();
    let iss = generate_isn(arp::our_ip(), remote_ip, local_port, remote_port);

    // Pre-resolve the next-hop MAC before any CONNECTIONS lock. Without this
    // the first SYN goes to L2 broadcast on a cold cache and is dropped by
    // most gateways; TCP retransmit kicks in 1 s later and only succeeds
    // once the cache passively learns the gateway MAC. Symptom in practice:
    // `debug <ip> <port>` needs 2–3 attempts on fresh boot, fixed by a prior
    // `ping`. ~500 ms cap; on timeout we still proceed and let TCP retry.
    let _ = arp::resolve(super::ipv4::arp_target_for(remote_ip), 50);

    let conn = TcpConn {
        state: State::SynSent,
        local_port,
        remote_ip,
        remote_port,
        snd_nxt: iss.wrapping_add(1),
        snd_una: iss,
        snd_iss: iss,
        rcv_nxt: 0,
        rcv_irs: 0,
        recv_buf: VecDeque::new(),
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        established: false,
        closed: false,
        error: false,
        wscale_ok: false,
        snd_wscale: 0,
    };

    // Find free slot
    let handle = {
        let mut conns = CONNECTIONS.lock();
        // Reclaim free OR fully-Closed slots. Without the Closed clause
        // a Closed conn pins its slot forever (tick_connections only
        // moves TimeWait→Closed, never frees it) — under browser churn
        // every slot ends up a Closed corpse and connect() starves.
        let slot = conns.iter()
            .position(|c| c.is_none())
            .or_else(|| conns.iter()
                .position(|c| matches!(c, Some(x) if x.state == State::Closed)))
            .ok_or(TcpError::TooManyConnections)?;
        conns[slot] = Some(conn);
        slot
    };

    // Send SYN
    send_syn(handle)?;

    // Wait for ESTABLISHED (blocking poll)
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        let conns = CONNECTIONS.lock();
        if let Some(ref c) = conns[handle] {
            if c.established { break; }
            if c.error {
                drop(conns);
                close_cleanup(handle);
                return Err(TcpError::ConnectionRefused);
            }
        } else {
            return Err(TcpError::ConnectionFailed);
        }
        drop(conns);

        if crate::interrupts::ticks() - t0 > 1000 { // 10s timeout
            close_cleanup(handle);
            return Err(TcpError::Timeout);
        }
        core::hint::spin_loop();
    }

    Ok(handle)
}

/// Listen on a local port. Returns handle. Use accept() to wait for connection.
#[allow(dead_code)]
pub fn listen(port: u16) -> Result<usize, TcpError> {
    let conn = TcpConn {
        state: State::Listen,
        local_port: port,
        remote_ip: [0; 4],
        remote_port: 0,
        snd_nxt: 0,
        snd_una: 0,
        snd_iss: 0,
        rcv_nxt: 0,
        rcv_irs: 0,
        recv_buf: VecDeque::new(),
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        established: false,
        closed: false,
        error: false,
        wscale_ok: false,
        snd_wscale: 0,
    };

    let mut conns = CONNECTIONS.lock();
    let slot = conns.iter().position(|c| c.is_none())
        .ok_or(TcpError::TooManyConnections)?;
    conns[slot] = Some(conn);
    Ok(slot)
}

/// Wait for an incoming connection on a listening handle. Blocking.
#[allow(dead_code)]
pub fn accept(handle: usize, timeout_ticks: u64) -> Result<(), TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        let conns = CONNECTIONS.lock();
        if let Some(ref c) = conns[handle] {
            if c.established { return Ok(()); }
            if c.error || c.closed {
                drop(conns);
                return Err(TcpError::ConnectionFailed);
            }
        } else {
            return Err(TcpError::NotConnected);
        }
        drop(conns);

        if timeout_ticks > 0 && crate::interrupts::ticks() - t0 > timeout_ticks {
            return Err(TcpError::Timeout);
        }
        core::hint::spin_loop();
    }
}

/// Check if a listening handle has an established connection (non-blocking).
#[allow(dead_code)]
pub fn is_established(handle: usize) -> bool {
    let conns = CONNECTIONS.lock();
    conns[handle].as_ref().map_or(false, |c| c.established)
}

/// Reset a connection back to Listen state (for accepting next client).
#[allow(dead_code)]
pub fn reset_to_listen(handle: usize) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    let port = conn.local_port;

    *conn = TcpConn {
        state: State::Listen,
        local_port: port,
        remote_ip: [0; 4],
        remote_port: 0,
        snd_nxt: 0,
        snd_una: 0,
        snd_iss: 0,
        rcv_nxt: 0,
        rcv_irs: 0,
        recv_buf: VecDeque::new(),
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        established: false,
        closed: false,
        error: false,
        wscale_ok: false,
        snd_wscale: 0,
    };
    Ok(())
}

/// Send data on a connection. Buffers and sends immediately (no Nagle).
pub fn send(handle: usize, data: &[u8]) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    if conn.state != State::Established { return Err(TcpError::NotConnected); }

    // Send in MSS-sized chunks immediately (no Nagle)
    let remote_ip = conn.remote_ip;
    let remote_port = conn.remote_port;
    let local_port = conn.local_port;

    for chunk in data.chunks(MSS as usize) {
        let seq = conn.snd_nxt;
        conn.snd_nxt = conn.snd_nxt.wrapping_add(chunk.len() as u32);
        conn.last_send_tick = crate::interrupts::ticks();

        send_segment(
            remote_ip, local_port, remote_port,
            seq, conn.rcv_nxt, ACK | PSH, recv_window(conn), chunk,
        );
    }

    Ok(())
}

/// Receive data. Returns available data (may be empty if nothing received yet).
/// Sends a window update ACK if significant buffer space was freed.
pub fn recv(handle: usize, buf: &mut [u8]) -> Result<usize, TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;

    let available = conn.recv_buf.len().min(buf.len());
    // Bulk copy out of the ring buffer instead of byte-by-byte pop_front
    // (that was ~112M pop_front/s at 100 MB/s — pure call overhead). The
    // VecDeque exposes its contents as up to two contiguous slices; memcpy
    // each, then drain in one shot.
    {
        let (a, b) = conn.recv_buf.as_slices();
        let na = a.len().min(available);
        buf[..na].copy_from_slice(&a[..na]);
        if na < available {
            buf[na..available].copy_from_slice(&b[..available - na]);
        }
        conn.recv_buf.drain(..available);
    }

    // Window-update ACK on every drain.
    //
    // The old code only ACKed when a SINGLE recv() freed >25 % of
    // the buffer (>16 KiB). But `recv_exact` (TLS) drains in small
    // increments, so that threshold was almost never met on a
    // sender that trickles (codeload generates tarballs on the
    // fly). The receive window then collapsed to 0 and only the
    // peer's exponentially-backing-off zero-window probe reopened
    // it → a fixed, host-independent ~31 KiB/s sawtooth. Static
    // CDNs (raw.githubusercontent) happened to deliver clean
    // ≥16 KiB bursts so the threshold fired and they ran ~100×
    // faster on the very same code path — that asymmetry was the
    // tell.
    //
    // Acknowledging as we consume is exactly what TCP is supposed
    // to do. recv() is called at ~TLS-record granularity (~16 KiB),
    // so this is one bare ACK per record — normal ACK density, not
    // a flood.
    if available > 0 && conn.state == State::Established {
        send_segment(
            conn.remote_ip, conn.local_port, conn.remote_port,
            conn.snd_nxt, conn.rcv_nxt, ACK, recv_window(conn), &[],
        );
        conn.ack_pending = false;
    }

    Ok(available)
}

/// Receive with blocking wait (polls until data or timeout).
pub fn recv_blocking(handle: usize, buf: &mut [u8], timeout_ticks: u64) -> Result<usize, TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        let n = recv(handle, buf)?;
        if n > 0 { return Ok(n); }

        // Check if connection closed
        {
            let conns = CONNECTIONS.lock();
            if let Some(ref c) = conns[handle] {
                if c.closed || c.error { return Ok(0); }
            } else {
                return Err(TcpError::NotConnected);
            }
        }

        if crate::interrupts::ticks() - t0 > timeout_ticks {
            return Ok(0);
        }
        core::hint::spin_loop();
    }
}

/// Close a connection gracefully (sends FIN).
pub fn close(handle: usize) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;

    if conn.state == State::Established {
        let seq = conn.snd_nxt;
        conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
        conn.state = State::FinWait1;

        send_segment(
            conn.remote_ip, conn.local_port, conn.remote_port,
            seq, conn.rcv_nxt, FIN | ACK, 0, &[],
        );
    }
    drop(conns);

    // Wait briefly for FIN-ACK
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        let conns = CONNECTIONS.lock();
        match conns[handle].as_ref().map(|c| c.state) {
            Some(State::TimeWait) | Some(State::Closed) | None => break,
            _ => {}
        }
        drop(conns);
        if crate::interrupts::ticks() - t0 > 200 { break; } // 2s
        core::hint::spin_loop();
    }

    close_cleanup(handle);
    Ok(())
}

/// Handle incoming TCP segment (called from ipv4)
pub fn handle_tcp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < HEADER_LEN { return; }

    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let data_offset = ((data[12] >> 4) as usize) * 4;
    let flags = data[13];
    let _window = u16::from_be_bytes([data[14], data[15]]);

    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();
    let payload = if data_offset < data.len() { &data[data_offset..] } else { &[] };

    let mut conns = CONNECTIONS.lock();

    // Find matching connection
    let idx = conns.iter().position(|c| {
        c.as_ref().map_or(false, |c|
            c.local_port == dst_port && c.remote_port == src_port && c.remote_ip == src_ip
        )
    });

    let idx = match idx {
        Some(i) => i,
        None => {
            // Check for a listener on this port
            if flags & SYN != 0 {
                let listen_idx = conns.iter().position(|c| {
                    c.as_ref().map_or(false, |c|
                        c.local_port == dst_port && c.state == State::Listen
                    )
                });
                if let Some(li) = listen_idx {
                    // Accept the SYN on the listening socket
                    let iss = generate_isn(arp::our_ip(), src_ip, dst_port, src_port);
                    let peer_ws = parse_wscale(data, data_offset);
                    let conn = conns[li].as_mut().unwrap();
                    conn.state = State::SynReceived;
                    conn.remote_ip = src_ip;
                    conn.remote_port = src_port;
                    conn.rcv_irs = seq;
                    conn.rcv_nxt = seq.wrapping_add(1);
                    conn.snd_iss = iss;
                    conn.snd_nxt = iss.wrapping_add(1);
                    conn.snd_una = iss;
                    conn.last_send_tick = crate::interrupts::ticks();
                    // Scaling is active only if the peer offered it too.
                    conn.wscale_ok = peer_ws.is_some();
                    conn.snd_wscale = peer_ws.unwrap_or(0);

                    // SYN-ACK: MSS, and Window Scale only if the peer asked for it.
                    let opts: &[u8] = if peer_ws.is_some() {
                        &[2, 4, (MSS >> 8) as u8, MSS as u8, 1, 3, 3, OUR_WSCALE]
                    } else {
                        &[2, 4, (MSS >> 8) as u8, MSS as u8]
                    };
                    send_segment_with_opts(
                        src_ip, dst_port, src_port,
                        iss, seq.wrapping_add(1), SYN | ACK, INITIAL_WINDOW, &[], opts,
                    );
                    return;
                }
            }
            // No connection and no listener: send RST if not RST
            if flags & RST == 0 {
                send_segment(src_ip, dst_port, src_port, ack, seq.wrapping_add(1), RST | ACK, 0, &[]);
            }
            return;
        }
    };

    let conn = conns[idx].as_mut().unwrap();

    // RST handling
    if flags & RST != 0 {
        conn.error = true;
        conn.state = State::Closed;
        return;
    }

    match conn.state {
        State::SynReceived => {
            // Waiting for ACK of our SYN-ACK
            if flags & ACK != 0 {
                conn.snd_una = ack;
                conn.state = State::Established;
                conn.established = true;
            }
        }

        State::SynSent => {
            if flags & SYN != 0 && flags & ACK != 0 {
                // SYN-ACK received
                conn.rcv_irs = seq;
                conn.rcv_nxt = seq.wrapping_add(1);
                conn.snd_una = ack;
                conn.state = State::Established;
                conn.established = true;

                // We always offer WScale in our SYN, so scaling is active iff
                // the SYN-ACK carries it. Set before the ACK so it advertises
                // the scaled window immediately.
                if let Some(ws) = parse_wscale(data, data_offset) {
                    conn.snd_wscale = ws;
                    conn.wscale_ok = true;
                }

                // Send ACK with full window
                send_segment(
                    conn.remote_ip, conn.local_port, conn.remote_port,
                    conn.snd_nxt, conn.rcv_nxt, ACK, recv_window(conn), &[],
                );
            }
        }

        State::Established => {
            // ACK processing
            if flags & ACK != 0 {
                if ack_in_range(conn.snd_una, ack, conn.snd_nxt) {
                    conn.snd_una = ack;
                }
            }

            // Data processing
            if !payload.is_empty() && seq == conn.rcv_nxt {
                let space = RECV_BUF_SIZE - conn.recv_buf.len();
                let copy = payload.len().min(space);
                // Bulk append — NOT byte-by-byte push_back (that was ~87M
                // push_back/s at ~700 Mbit). extend reserves once + copies.
                conn.recv_buf.extend(payload[..copy].iter().copied());
                conn.rcv_nxt = conn.rcv_nxt.wrapping_add(copy as u32);
                // Delayed ACK (RFC 1122): ACK every SECOND full-data segment,
                // not every one — halves the ~60k send_segment/s (each an alloc-
                // heavy packet build + TX) that pegged the RX core. A lone
                // pending ACK is flushed by the 40 ms timer in tick_connections.
                if conn.ack_pending {
                    send_segment(
                        conn.remote_ip, conn.local_port, conn.remote_port,
                        conn.snd_nxt, conn.rcv_nxt, ACK, recv_window(conn), &[],
                    );
                    conn.ack_pending = false;
                } else {
                    conn.ack_pending = true;
                    conn.ack_tick = crate::interrupts::ticks();
                }
            }

            // FIN from remote
            if flags & FIN != 0 {
                conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                conn.state = State::CloseWait;
                conn.closed = true;
                // ACK the FIN
                send_segment(
                    conn.remote_ip, conn.local_port, conn.remote_port,
                    conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[],
                );
            }

        }

        State::FinWait1 => {
            if flags & ACK != 0 {
                conn.snd_una = ack;
                if flags & FIN != 0 {
                    conn.rcv_nxt = seq.wrapping_add(1);
                    conn.state = State::TimeWait;
                    send_segment(
                        conn.remote_ip, conn.local_port, conn.remote_port,
                        conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[],
                    );
                } else {
                    conn.state = State::FinWait2;
                }
            }
        }

        State::FinWait2 => {
            if flags & FIN != 0 {
                conn.rcv_nxt = seq.wrapping_add(1);
                conn.state = State::TimeWait;
                send_segment(
                    conn.remote_ip, conn.local_port, conn.remote_port,
                    conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[],
                );
            }
        }

        State::LastAck => {
            if flags & ACK != 0 {
                conn.state = State::Closed;
            }
        }

        _ => {}
    }
}

/// Periodic tick: retransmit, delayed ACKs, timeouts
pub fn tick_connections() {
    let now = crate::interrupts::ticks();
    let mut conns = CONNECTIONS.lock();

    for slot in conns.iter_mut().flatten() {
        // Delayed ACK
        if slot.ack_pending && now - slot.ack_tick >= DELAYED_ACK_TICKS {
            send_segment(
                slot.remote_ip, slot.local_port, slot.remote_port,
                slot.snd_nxt, slot.rcv_nxt, ACK, recv_window(slot), &[],
            );
            slot.ack_pending = false;
        }

        // SYN retry
        if slot.state == State::SynSent {
            let retry_interval = RETRY_TICKS_BASE << slot.retries.min(4);
            if now - slot.last_send_tick > retry_interval {
                if slot.retries >= MAX_RETRIES {
                    slot.error = true;
                    slot.state = State::Closed;
                } else {
                    slot.retries += 1;
                    slot.last_send_tick = now;
                    send_segment(
                        slot.remote_ip, slot.local_port, slot.remote_port,
                        slot.snd_iss, 0, SYN, INITIAL_WINDOW, &[],
                    );
                }
            }
        }

        // TimeWait cleanup (2 seconds)
        if slot.state == State::TimeWait && now - slot.last_send_tick > 200 {
            slot.state = State::Closed;
        }
    }
}

// === Internal ===

fn send_syn(handle: usize) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    conn.last_send_tick = crate::interrupts::ticks();

    // SYN with MSS + Window Scale options (MSS, NOP, WScale = 8 bytes, aligned).
    let mut opts = [0u8; 8];
    opts[0] = 2;  // MSS option kind
    opts[1] = 4;  // MSS option length
    opts[2..4].copy_from_slice(&MSS.to_be_bytes());
    opts[4] = 1;            // NOP — align the 3-byte WScale to a 4-byte boundary
    opts[5] = 3;            // Window Scale option kind
    opts[6] = 3;            // length
    opts[7] = OUR_WSCALE;   // shift count

    send_segment_with_opts(
        conn.remote_ip, conn.local_port, conn.remote_port,
        conn.snd_iss, 0, SYN, INITIAL_WINDOW, &[], &opts,
    );
    Ok(())
}

fn send_segment(
    dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8],
) {
    send_segment_with_opts(dst_ip, src_port, dst_port, seq, ack, flags, window, payload, &[]);
}

fn send_segment_with_opts(
    dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8], options: &[u8],
) {
    let opts_padded = (options.len() + 3) & !3; // pad to 4 bytes
    let header_len = HEADER_LEN + opts_padded;
    let total_len = header_len + payload.len();

    let mut pkt = alloc::vec![0u8; total_len];

    pkt[0..2].copy_from_slice(&src_port.to_be_bytes());
    pkt[2..4].copy_from_slice(&dst_port.to_be_bytes());
    pkt[4..8].copy_from_slice(&seq.to_be_bytes());
    pkt[8..12].copy_from_slice(&ack.to_be_bytes());
    pkt[12] = ((header_len / 4) as u8) << 4; // data offset
    pkt[13] = flags;
    pkt[14..16].copy_from_slice(&window.to_be_bytes());

    // Options
    if !options.is_empty() {
        pkt[HEADER_LEN..HEADER_LEN + options.len()].copy_from_slice(options);
    }

    // Payload
    pkt[header_len..].copy_from_slice(payload);

    // TCP checksum (pseudo-header + TCP segment)
    let src_ip = arp::our_ip();
    let checksum = tcp_checksum(&src_ip, &dst_ip, &pkt);
    pkt[16..18].copy_from_slice(&checksum.to_be_bytes());

    ipv4::send(dst_ip, ipv4::PROTO_TCP, &pkt);
}

fn tcp_checksum(src_ip: &[u8; 4], dst_ip: &[u8; 4], segment: &[u8]) -> u16 {
    let mut sum = 0u32;

    // Pseudo-header
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += 6u32; // protocol TCP
    sum += segment.len() as u32;

    // TCP segment
    for i in (0..segment.len()).step_by(2) {
        let word = if i + 1 < segment.len() {
            u16::from_be_bytes([segment[i], segment[i + 1]])
        } else {
            (segment[i] as u16) << 8
        };
        sum += word as u32;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Scan a segment's TCP options for the Window Scale option (kind 3) and
/// return its shift count. `data_offset` is the TCP header length in bytes.
fn parse_wscale(seg: &[u8], data_offset: usize) -> Option<u8> {
    let end = data_offset.min(seg.len());
    let mut i = HEADER_LEN;
    while i < end {
        match seg[i] {
            0 => break,        // End of Option List
            1 => i += 1,       // NOP
            kind => {
                if i + 1 >= end { break; }
                let len = seg[i + 1] as usize;
                if len < 2 { break; } // malformed
                if kind == 3 && len == 3 && i + 2 < end {
                    return Some(seg[i + 2]);
                }
                i += len;
            }
        }
    }
    None
}

fn ack_in_range(una: u32, ack: u32, nxt: u32) -> bool {
    // Check if ack is within (una, nxt] accounting for wrapping
    let diff_una = ack.wrapping_sub(una);
    let diff_nxt = nxt.wrapping_sub(una);
    diff_una > 0 && diff_una <= diff_nxt
}

fn close_cleanup(handle: usize) {
    CONNECTIONS.lock()[handle] = None;
}

/// List active connections for netstat display
/// Returns (snd_nxt - snd_una, recv_buf.len()) for a given connection
/// — "bytes in flight" + "bytes buffered ready for us to read".
/// Diagnostic-only.
pub fn debug_progress(handle: usize) -> Option<(u32, usize)> {
    let conns = CONNECTIONS.lock();
    let c = conns[handle].as_ref()?;
    Some((c.snd_nxt.wrapping_sub(c.snd_una), c.recv_buf.len()))
}

pub fn list_connections() -> alloc::vec::Vec<(u16, [u8; 4], u16, &'static str)> {
    let conns = CONNECTIONS.lock();
    let mut result = alloc::vec::Vec::new();
    for slot in conns.iter().flatten() {
        let state_str = match slot.state {
            State::Closed => "CLOSED",
            State::Listen => "LISTEN",
            State::SynReceived => "SYN_RCVD",
            State::SynSent => "SYN_SENT",
            State::Established => "ESTABLISHED",
            State::FinWait1 => "FIN_WAIT_1",
            State::FinWait2 => "FIN_WAIT_2",
            State::CloseWait => "CLOSE_WAIT",
            State::LastAck => "LAST_ACK",
            State::TimeWait => "TIME_WAIT",
        };
        result.push((slot.local_port, slot.remote_ip, slot.remote_port, state_str));
    }
    result
}

#[derive(Debug)]
pub enum TcpError {
    TooManyConnections,
    ConnectionRefused,
    ConnectionFailed,
    NotConnected,
    Timeout,
}

impl core::fmt::Display for TcpError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            TcpError::TooManyConnections => write!(f, "too many connections"),
            TcpError::ConnectionRefused => write!(f, "connection refused"),
            TcpError::ConnectionFailed => write!(f, "connection failed"),
            TcpError::NotConnected => write!(f, "not connected"),
            TcpError::Timeout => write!(f, "connection timed out"),
        }
    }
}
