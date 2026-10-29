//! TCP — the kernel's side of `net_core::tcp`: the connection table behind
//! a lock, the machine it runs on, and the waits that drive the stack while
//! they block.

use core::sync::atomic::Ordering;
use spin::{Mutex, MutexGuard, Once};
use net_core::tcp::{self as core_tcp, Env, Stack, Stats};
use super::{ipv4, arp};

pub use net_core::tcp::{Owner, KERNEL, TcpError, TcpInfo};

/// The stack's counters.
pub static STATS: Stats = Stats::new();

static STACK: Once<Mutex<Stack>> = Once::new();

/// The table, created at first use with a per-boot ISN secret (RFC 6528).
fn stack() -> MutexGuard<'static, Stack> {
    STACK.call_once(|| Mutex::new(Stack::new(
        crate::csprng::random_256(), &STATS, crate::memory::usable_bytes()))).lock()
}

/// The machine under the stack.
struct KernelEnv;

impl Env for KernelEnv {
    fn ticks(&self) -> u64 { crate::interrupts::ticks() }
    fn uptime_us(&self) -> u64 { crate::interrupts::uptime_us() }
    fn our_ip(&self) -> [u8; 4] { arp::our_ip() }
    fn random_u64(&mut self) -> u64 { crate::csprng::random_u64() }
    fn free_mib(&self) -> usize { crate::memory::stats().1 }
    fn ip_send(&mut self, dst: [u8; 4], segment: &[u8], id: u16) -> bool {
        ipv4::send_tcp(dst, segment, id)
    }
    fn mtu(&self, dst: [u8; 4]) -> u32 { ipv4::path_mtu(dst) }
    fn update_pmtu(&mut self, dst: [u8; 4], mtu: u32) { ipv4::update_pmtu(dst, mtu) }
}

/// Empty looks at `ack_gen` before yielding.
///
/// The ACK comes from the driver fiber on another core; yielding here
/// means missing it by a scheduler pass. 4096 spins are a few
/// microseconds, shorter than any realistic RTT.
const SEND_SPIN_BUDGET: u32 = 4096;

/// Yield rounds without a wakeup after which we try again anyway. Costs
/// nothing normally (the wakeup comes long before) and turns a dead wait
/// into a slow one.
const RETRY_ROUNDS: u32 = 16;

/// (window bytes, srtt ms, cap bytes) of the last computation.
pub fn window_diag() -> (u32, u32, u32) {
    use core::sync::atomic::Ordering::Relaxed;
    (STATS.wnd_last.load(Relaxed), STATS.wnd_srtt.load(Relaxed), STATS.wnd_cap.load(Relaxed))
}

/// `net window <KB>` sets the cap, `net window auto` removes it.
pub fn set_rcv_window_force(bytes: u32) {
    STATS.rcv_wnd_force.store(bytes, Ordering::Relaxed);
}

/// Current manual cap in bytes (0 = off).
pub fn rcv_window_force() -> u32 {
    STATS.rcv_wnd_force.load(Ordering::Relaxed)
}

/// Segments refused for `tcp_mem` since the last call.
pub fn take_mem_drops() -> u32 {
    STATS.mem_drops.swap(0, Ordering::Relaxed)
}

/// (ahead, behind) out-of-order segment counts since the last call; resets both.
pub fn take_ooo_stats() -> (u32, u32) {
    (STATS.ooo_ahead.swap(0, Ordering::Relaxed), STATS.ooo_behind.swap(0, Ordering::Relaxed))
}

/// Max recv-buffer depth (bytes) seen since the last call; resets to 0.
pub fn take_max_rxbuf() -> usize {
    STATS.max_rxbuf.swap(0, Ordering::Relaxed)
}

/// Count of connection-originated segments sent since the last call; resets.
pub fn take_tx_segs() -> u32 {
    STATS.tx_segs.swap(0, Ordering::Relaxed)
}

/// Open a TCP connection without waiting for the handshake: the handle comes
/// back at once, the caller asks `connect_status` until it answers.
///
/// This is the form modules get. A blocking wait inside a host call freezes
/// every other fiber on that worker core — including the WiFi driver, whose
/// card then goes unpolled for the whole wait (the RB pool holds milliseconds).
/// `fiber::pump_peers` cannot cover it: it returns early when called from
/// inside a fiber, and a module is a fiber.
pub fn connect_start_as(owner: Owner, remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {
    // tcp_v4_connect: no route, or one to a broadcast or multicast address,
    // is ENETUNREACH at once rather than a SYN that times out.
    if !ipv4::iface().output_ok(remote_ip, true) {
        return Err(TcpError::Unreachable);
    }
    stack().connect_start(&mut KernelEnv, owner, remote_ip, remote_port)
}

/// Connection state: 1 = usable, 0 = still handshaking, -1 = the peer hung up
/// cleanly, -2 = it failed (reset, or we ran out of retransmits).
pub fn connect_status(handle: usize) -> i32 {
    stack().connect_status(handle)
}

/// Open a TCP connection, blocking until established. Native callers only —
/// they run as a task on a worker core, where `super::poll` pumps the peer
/// fibers so the NIC keeps being drained while we wait. A module must use
/// `connect_start` + `connect_status` instead; see the note there.
pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {
    connect_as(KERNEL, remote_ip, remote_port)
}

/// `connect` on behalf of `owner` (see `connect_start_as`).
pub fn connect_as(owner: Owner, remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {
    let handle = connect_start_as(owner, remote_ip, remote_port)?;

    // Wait for ESTABLISHED (blocking poll)
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        match connect_status(handle) {
            1 => break,
            // -2 is our own retry budget running out; -1 is the peer closing
            // the connection (a RST answers a SYN with a refusal). They call
            // for opposite investigations, so they get different errors.
            -2 => {
                // An ICMP error says why at once; without one our retries ran
                // out.
                let err = stack().conn_error(handle);
                if err.is_none() {
                    if let Some(c) = stack().conn_debug(handle) {
                        crate::kprintln!(
                            "[tcp] connect gave up: state {} syn_retries {}",
                            c.state, c.retries);
                    }
                }
                stack().remove(&mut KernelEnv, handle);
                return Err(match err {
                    Some(net_core::tcp::SockErr::Refused) => TcpError::ConnectionRefused,
                    Some(_) => TcpError::Unreachable,
                    None => TcpError::Timeout,
                });
            }
            n if n < 0 => {
                stack().remove(&mut KernelEnv, handle);
                return Err(TcpError::ConnectionRefused);
            }
            _ => {}
        }

        if crate::interrupts::ticks() - t0 > 1000 { // 10s timeout
            // Say how far it got: SYN never answered (`retries` climbing),
            // or the state machine stuck somewhere else.
            if let Some(c) = stack().conn_debug(handle) {
                crate::kprintln!(
                    "[tcp] connect timeout: state {} syn_retries {}",
                    c.state, c.retries);
            }
            stack().remove(&mut KernelEnv, handle);
            return Err(TcpError::Timeout);
        }
        core::hint::spin_loop();
    }

    Ok(handle)
}

/// Listen on a local port with room for `backlog` connections not yet
/// accepted. Returns the listener's handle.
#[allow(dead_code)]
pub fn listen(port: u16, backlog: u32) -> Result<usize, TcpError> {
    stack().listen(port, backlog)
}

/// Wait for an incoming connection on a listener and return its handle.
/// Blocking; `timeout_ticks` 0 waits forever.
#[allow(dead_code)]
pub fn accept(handle: usize, timeout_ticks: u64) -> Result<usize, TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        if let Some(child) = stack().accept(handle)? {
            return Ok(child);
        }

        if timeout_ticks > 0 && crate::interrupts::ticks() - t0 > timeout_ticks {
            return Err(TcpError::Timeout);
        }
        core::hint::spin_loop();
    }
}

/// Send data on a connection. Buffers and sends immediately (no Nagle).
///
/// The bytes are also kept in the send buffer until the peer acknowledges
/// them, so `tick_connections` can retransmit. Returns `WouldBlock` when too
/// much is already unacknowledged — that is real backpressure, not an error.
pub fn send(handle: usize, data: &[u8]) -> Result<(), TcpError> {
    let t_enter = crate::interrupts::rdtsc();
    let r = stack().send(&mut KernelEnv, handle, data);
    let dt = crate::interrupts::rdtsc().wrapping_sub(t_enter);
    if r.is_err() {
        STATS.send_wouldblock.fetch_add(1, Ordering::Relaxed);
        STATS.send_blocked_tsc.fetch_add(dt, Ordering::Relaxed);
    } else {
        STATS.send_tsc.fetch_add(dt, Ordering::Relaxed);
    }
    r
}

/// How far the send buffer has grown (with `snd_wnd_of`, what the peer last
/// offered). Diagnostics: shows whether `tcp_sndbuf_expand` took effect.
pub fn snd_limit_of(handle: usize) -> usize {
    stack().snd_limit_of(handle)
}

/// Bytes currently unacknowledged in the send buffer.
pub fn snd_unacked_of(handle: usize) -> usize {
    stack().snd_unacked_of(handle)
}

/// (cwnd in packets, ssthresh, duplicate ACKs, in recovery)
pub fn cwnd_of(handle: usize) -> (u32, u32, u32, bool) {
    stack().cwnd_of(handle)
}

pub fn snd_wnd_of(handle: usize) -> usize {
    stack().snd_wnd_of(handle)
}

/// Reset the counters so a measurement sees only its own run.
pub fn send_stats_reset() {
    STATS.send_tsc.store(0, Ordering::Relaxed);
    STATS.send_blocked_tsc.store(0, Ordering::Relaxed);
    STATS.send_segs.store(0, Ordering::Relaxed);
    STATS.send_wouldblock.store(0, Ordering::Relaxed);
    STATS.send_maxbuf.store(0, Ordering::Relaxed);
    STATS.send_refused.store(0, Ordering::Relaxed);
    STATS.fast_retrans.store(0, Ordering::Relaxed);
    STATS.rto_fired.store(0, Ordering::Relaxed);
    STATS.dupacks_seen.store(0, Ordering::Relaxed);
}

/// (TSC in send, TSC in refused send, segments, WouldBlock, largest
/// send_buf)
pub fn send_stats() -> (u64, u64, u64, u64, u64) {
    (STATS.send_tsc.load(Ordering::Relaxed),
     STATS.send_blocked_tsc.load(Ordering::Relaxed),
     STATS.send_segs.load(Ordering::Relaxed),
     STATS.send_wouldblock.load(Ordering::Relaxed),
     STATS.send_maxbuf.load(Ordering::Relaxed))
}

/// Send, waiting out backpressure. Native callers only — the same rule as
/// `connect`: this polls, which pumps the peer fibers on a worker core. A
/// module must handle `WouldBlock` itself and sleep between tries.
pub fn send_blocking(handle: usize, data: &[u8], timeout_ticks: u64) -> Result<(), TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        // Read before the attempt, not after: an ACK arriving during `send()`
        // would otherwise be missed, and we would wait for the next one (or
        // the timeout). The classic lost wakeup.
        let zuletzt = STATS.ack_gen.load(Ordering::Relaxed);
        match send(handle, data) {
            Err(TcpError::WouldBlock) => {}
            other => return other,
        }
        // A second attempt is only worth it once an ACK has made room.
        // Retrying `send()` in a loop would hammer the table's lock, the
        // lock the ACK itself needs.
        //
        // `yield_ready` returns `false` outside a fiber (Core 0, OTA); there
        // we drive the stack ourselves, or the ACK would never arrive.
        //
        // Spin briefly on the atomic counter, then yield: yielding blindly
        // costs a scheduler pass per ACK. The same shape as the driver's
        // receive path.
        let mut leer = 0u32;
        let mut runden = 0u32;
        while STATS.ack_gen.load(Ordering::Relaxed) == zuletzt {
            if leer < SEND_SPIN_BUDGET {
                leer += 1;
                core::hint::spin_loop();
                continue;
            }
            leer = 0;
            if crate::interrupts::ticks() - t0 > timeout_ticks {
                return Err(TcpError::Timeout);
            }
            // Drive the stack in case nobody else does — on core 0 there
            // is no driver fiber to bring the ACK in.
            super::poll();
            tick_connections();
            crate::smp::fiber::yield_ready();
            // Never wait on the counter forever: a missed wakeup (e.g. a
            // window update that does not bump it) must make the wait slow,
            // not dead, so retry after a bounded number of rounds.
            runden += 1;
            if runden >= RETRY_ROUNDS {
                break;
            }
        }
    }
}

/// Receive data. Returns available data (may be empty if nothing received yet).
/// Sends a window update ACK if significant buffer space was freed.
pub fn recv(handle: usize, buf: &mut [u8]) -> Result<usize, TcpError> {
    stack().recv(&mut KernelEnv, handle, buf)
}

/// Receive with blocking wait (polls until data or timeout).
pub fn recv_blocking(handle: usize, buf: &mut [u8], timeout_ticks: u64) -> Result<usize, TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        // NIC-drain only (the TLS / OTA-https recv hot path). The full
        // super::poll() at spin rate would contend the table's lock with
        // actual packet processing; poll_rx_only throttles the TCP timers.
        super::poll_rx_only();

        let n = recv(handle, buf)?;
        if n > 0 { return Ok(n); }

        // Check if connection closed
        if stack().recv_done(handle)? { return Ok(0); }

        if crate::interrupts::ticks() - t0 > timeout_ticks {
            // `Err(Timeout)`, not `Ok(0)`: `Ok(0)` is how a caller learns the
            // peer hung up, and a link that merely went quiet must not read as
            // end-of-file. A caller that can distinguish the two can wait
            // longer.
            return Err(TcpError::Timeout);
        }
        // Timer-NAPI: HLT instead of spinning, so the core is not pegged.
        // Records the halt so `cores` is accurate. Wakes on the per-core timer;
        // the NIC re-fills the ring in the gap.
        crate::interrupts::worker_idle_hlt();
    }
}

/// True if `handle` is an established, un-closed, un-errored connection —
/// i.e. safe to send another request on (HTTP keep-alive reuse).
pub fn conn_healthy(handle: usize) -> bool {
    stack().conn_healthy(handle)
}

/// Where this connection goes. For connection coalescing: two names may
/// share a connection only if they lead to the same address.
pub fn peer(handle: usize) -> Option<([u8; 4], u16)> {
    stack().peer(handle)
}

/// Close a connection gracefully (sends FIN) and return at once.
///
/// Waiting here would spin on the caller's core for every `Connection:
/// close` peer, and a host call that spins also freezes every other fiber on
/// that worker core.
pub fn close(handle: usize) -> Result<(), TcpError> {
    stack().close(&mut KernelEnv, handle)
}

/// An ICMP error about one of our segments (from `icmp`).
pub fn icmp_error(e: &net_core::ip::IcmpError) {
    stack().icmp_error(&mut KernelEnv, e);
}

/// Handle incoming TCP segment (called from ipv4)
pub fn handle_tcp(ip_packet: &[u8], data: &[u8]) {
    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();
    stack().input(&mut KernelEnv, src_ip, data);
}

/// Periodic tick: retransmit, delayed ACKs, timeouts
pub fn tick_connections() {
    // The segments go out after the table is let go: holding it across the
    // TX doorbell would block worker-core `recv` behind Core-0's periodic
    // ACKs/retries.
    let pending = stack().tick(&mut KernelEnv);
    for p in &pending {
        core_tcp::send_pending(&mut KernelEnv, &STATS, p);
    }
}

/// Does any connection run a timer (retransmit, delayed ACK, SYN retry,
/// TIME_WAIT, FIN timeout)? The shell loop drives `tick_connections` and
/// must keep coming back while this holds.
pub fn has_timers() -> bool {
    stack().has_timers()
}

/// Every connection, with `tcp_get_info` for the full ones.
pub fn connections() -> alloc::vec::Vec<core_tcp::ConnRow> {
    stack().connections(&KernelEnv)
}
