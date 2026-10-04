//! debug — reverse debug shell (WASM module)
//!
//! Mirrors the window's terminal over TCP to a `nc -l <port>` listener
//! on the developer's machine. Dials out (reverse-shell style), no auth,
//! no crypto — a stopgap until real SSH.
//!
//! Usage:  run debug <ip> <port>
//! On laptop:  nc -lk 22222

#![no_std]

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

mod host;

/// One source for the version, printed in the banner.
const VERSION: &str = "0.7.0";

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// `debug` needs the network: it writes its log over a raw TCP socket to
/// an `nc -lk` on the developer's machine. The `npk_tcp_*` calls check
/// the NET right like for any other module.
#[unsafe(link_section = ".npk.caps")]
#[used]
// READ is listed because the default without a section is
// `READ | EXECUTE | RENDER` — a section replaces the default and must
// name everything it wants to keep.
static NPK_CAPS: [u8; 2] = [0x01 | 0x04 | 0x08, 0x01];   // READ|EXEC|RENDER, ext: NET

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // The banner carries the version: a mirror that goes quiet looks
    // exactly like a broken connection, so which build is running is the
    // first question.
    host::print("[debug] reverse-mirror ");
    host::print(VERSION);
    host::print(" (global mirror)\n");

    let ip = host::target_ip();
    let port = host::target_port();
    if ip == 0 || port == 0 {
        host::print("[debug] no target set. Usage: run debug <ip> <port>\n");
        return;
    }

    let my_term = host::self_terminal();
    if my_term < 0 {
        host::print("[debug] no terminal\n");
        return;
    }

    host::print("[debug] target ");
    host::print_ip(ip);
    host::print(":");
    host::print_dec(port as u32);
    host::print(" (mirror term ");
    host::print_dec(my_term as u32);
    host::print(")\n");

    // Open the everything-sink (-1): every write, whichever terminal the
    // kernel routed it to. Bound to one terminal the mirror would go quiet
    // whenever output is redirected — background messages go to the
    // primary loop, a command's output to the loop it was typed in. Kernels
    // that do not know -1 fall back to our own terminal.
    let mut sink = -1i32;
    if host::stream_open(sink) != 0 {
        sink = my_term;
        if host::stream_open(sink) != 0 {
            host::print("[debug] stream_open failed\n");
            return;
        }
        host::print("[debug] kernel has no global mirror - only this terminal\n");
    }

    // Dial, then wait outside the host call. This module is a fiber, so a
    // connect that blocks in the kernel would freeze every other fiber on
    // the same worker core, drivers included; sleeping between polls
    // leaves the core.
    // Outer loop: a mirror that gives up the moment the link hiccups is
    // useless exactly when it is needed. The far end must keep listening too
    // (`nc -lk`, or `while true; do nc -l PORT; done`) — plain `nc -l` takes
    // one connection and exits.
    //
    // The sink stays open across reconnects, so output produced while we were
    // away is still in its 64 KB ring when we come back.
    let mut attempt: u32 = 0;
    loop {
    let sock = host::tcp_connect(ip, port);
    let mut connected = sock >= 0;
    if connected {
        // 10 s at 20 ms.
        connected = false;
        for _ in 0..500 {
            match host::tcp_status(sock) {
                1 => { connected = true; break; }
                0 => host::sleep(20),
                _ => break,
            }
        }
    }
    if !connected {
        if sock >= 0 { host::tcp_close(sock); }
        attempt = attempt.saturating_add(1);
        // Say it the first few times, then stop — the retry itself is silent
        // work and this print goes to the local screen.
        if attempt <= 3 {
            host::print("[debug] connect failed (is `nc -lk ");
            host::print_dec(port as u32);
            host::print("` running?) — retrying\n");
        }
        // 2 s, 4 s, 8 s … capped at 30 s.
        let back = 2000u32.saturating_mul(1 << attempt.min(4)) / 2;
        host::sleep(back.min(30000) as i32);
        continue;
    }
    attempt = 0;
    // Say who we are after the sink and the socket exist — the banner above
    // is printed before either, so it only reaches the local screen. Which
    // version is running, and whether it got the global mirror, has to be
    // answerable from the far end.
    host::print("[debug] connected — reverse-mirror ");
    host::print(VERSION);
    host::print(if sink < 0 { " (global mirror)\n" } else { " (single terminal only)\n" });

    // Relay loop. Poll both directions with a short sleep to yield the core.
    let mut tx_buf = [0u8; 1024];
    let mut rx_buf = [0u8; 256];
    let mut idle_rounds: u32 = 0;
    let mut dropped: u32 = 0;
    let mut round: u32 = 0;

    loop {
        let mut did_work = false;

        // Did the far end hang up? `tcp_recv` never says so — it just returns
        // 0 bytes forever in CloseWait.
        // Every 16th round only: this takes the kernel's CONNECTIONS lock,
        // which the RX path takes very often under load; the answer changes
        // once per session.
        round = round.wrapping_add(1);
        match if round % 16 == 0 { host::tcp_status(sock) } else { 1 } {
            1 => {}
            // -2 is the interesting one: the connection did not end, it failed —
            // five retransmits with no acknowledgement, i.e. the link went dead
            // for about six seconds. It gets its own message.
            -2 => {
                host::print("[debug] link went dead (no ACK for ~6 s) — disconnecting\n");
                break;
            }
            _ => {
                host::print("[debug] far end closed — disconnecting\n");
                break;
            }
        }

        // Terminal output → TCP
        let n = host::stream_read(sink, &mut tx_buf);
        if n > 0 {
            match host::tcp_send(sock, &tx_buf[..n as usize]) {
                0 => {}
                // -2 = too much unacknowledged. Not a failure: give the
                // retransmit a moment and offer the same bytes again, or the
                // mirror loses exactly the output the developer is waiting for.
                -2 => {
                    let mut tries = 0;
                    let mut sent = false;
                    while tries < 50 {
                        host::sleep(10);
                        match host::tcp_send(sock, &tx_buf[..n as usize]) {
                            0 => { sent = true; break; }
                            -2 => tries += 1,
                            _ => break,
                        }
                    }
                    if !sent {
                        // Backpressure is not failure. A mirror may lose lines
                        // — the sink ring already drops the oldest on overflow
                        // — but it must never disconnect over it, or a saturating
                        // download would shut the mirror down without a reason
                        // reaching the far end.
                        dropped = dropped.saturating_add(1);
                        if dropped == 1 {
                            host::print("[debug] mirror behind — dropping output, staying connected\n");
                        }
                    }
                }
                _ => {
                    host::print("[debug] tcp_send failed — closing\n");
                    break;
                }
            }
            did_work = true;
        }

        // TCP input → keyboard inject
        let n = host::tcp_recv(sock, &mut rx_buf);
        if n < 0 {
            host::print("[debug] tcp_recv error — closing\n");
            break;
        }
        if n > 0 {
            for i in 0..(n as usize) {
                host::key_inject(rx_buf[i]);
            }
            did_work = true;
        }

        // Adaptive sleep: brief when active, longer when idle.
        if did_work {
            idle_rounds = 0;
        } else {
            idle_rounds = idle_rounds.saturating_add(1);
            let ms = if idle_rounds < 10 { 5 } else if idle_rounds < 100 { 20 } else { 100 };
            host::sleep(ms);
        }
    }

    // Relay ended. Close our side and dial again — the reason was already
    // printed above, and how long the gap lasted is visible from the two
    // timestamps in the surrounding log.
    host::tcp_close(sock);
    host::print("[debug] reconnecting in 2 s\n");
    host::sleep(2000);
    }
}
