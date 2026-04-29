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

const MAX_CONNECTIONS: usize = 16;
const MSS: u16 = 1460; // standard Ethernet MSS
const INITIAL_WINDOW: u16 = 65535; // Maximum TCP window (no window scaling)

/// Calculate current receive window based on free buffer space.
fn recv_window(conn: &TcpConn) -> u16 {
    let free = RECV_BUF_SIZE.saturating_sub(conn.recv_buf.len());
    (free as u16).min(INITIAL_WINDOW)
}
const MAX_RETRIES: u8 = 3;
const RETRY_TICKS_BASE: u64 = 100; // 1 second (100Hz)
const RECV_BUF_SIZE: usize = 65535;
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
        recv_buf: VecDeque::with_capacity(RECV_BUF_SIZE),
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        established: false,
        closed: false,
        error: false,
    };

    // Find free slot
    let handle = {
        let mut conns = CONNECTIONS.lock();
        let slot = conns.iter().position(|c| c.is_none())
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
        recv_buf: VecDeque::with_capacity(RECV_BUF_SIZE),
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        established: false,
        closed: false,
        error: false,
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
        recv_buf: VecDeque::with_capacity(RECV_BUF_SIZE),
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        established: false,
        closed: false,
        error: false,
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

    let before = conn.recv_buf.len();
    let available = before.min(buf.len());
    for i in 0..available {
        buf[i] = conn.recv_buf.pop_front().unwrap();
    }

    // Send window update if we freed significant space (>25% of buffer)
    if available > 0 && conn.state == State::Established {
        let freed = available;
        if freed > RECV_BUF_SIZE / 4 || (before >= RECV_BUF_SIZE * 3 / 4 && conn.recv_buf.len() < RECV_BUF_SIZE / 2) {
            send_segment(
                conn.remote_ip, conn.local_port, conn.remote_port,
                conn.snd_nxt, conn.rcv_nxt, ACK, recv_window(conn), &[],
            );
        }
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

                    // Send SYN-ACK with MSS option
                    let mut opts = [0u8; 4];
                    opts[0] = 2;  // MSS option kind
                    opts[1] = 4;  // MSS option length
                    opts[2..4].copy_from_slice(&MSS.to_be_bytes());

                    send_segment_with_opts(
                        src_ip, dst_port, src_port,
                        iss, seq.wrapping_add(1), SYN | ACK, INITIAL_WINDOW, &[], &opts,
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
                for &b in &payload[..copy] {
                    conn.recv_buf.push_back(b);
                }
                conn.rcv_nxt = conn.rcv_nxt.wrapping_add(copy as u32);
                conn.ack_pending = true;
                conn.ack_tick = crate::interrupts::ticks();
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

            // Send ACK immediately for received data (with dynamic window)
            if conn.ack_pending && !payload.is_empty() {
                send_segment(
                    conn.remote_ip, conn.local_port, conn.remote_port,
                    conn.snd_nxt, conn.rcv_nxt, ACK, recv_window(conn), &[],
                );
                conn.ack_pending = false;
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

    // SYN with MSS option
    let mut opts = [0u8; 4];
    opts[0] = 2;  // MSS option kind
    opts[1] = 4;  // MSS option length
    opts[2..4].copy_from_slice(&MSS.to_be_bytes());

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
