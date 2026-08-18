//! debug — reverse debug shell (WASM module)
//!
//! Mirrors the window's terminal over TCP to a `nc -l <port>` listener
//! on the developer's machine. Dials out (reverse-shell style), no auth,
//! no crypto — feature is temporary, will be replaced by real SSH later.
//!
//! Usage:  run debug <ip> <port>
//! On laptop:  nc -l 22222

#![no_std]

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

mod host;

/// One source for the version, printed in the banner.
const VERSION: &str = "0.6.0";

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // The banner carries the version because the failure mode of the OLD one —
    // mirroring a single terminal, so the console goes quiet the moment output
    // is routed elsewhere — looks exactly like a broken connection. Knowing
    // which one is running is the first question, every time.
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

    // Open the everything-sink (-1): every write, whichever terminal the kernel
    // routed it to. Bound to one index this mirror went quiet the moment output
    // was redirected — background messages go to the primary loop, a command's
    // output to the loop it was typed in — and it looked like the machine had
    // stopped answering while it was still printing. Older kernels do not know
    // -1, so fall back to our own terminal.
    let mut sink = -1i32;
    if host::stream_open(sink) != 0 {
        sink = my_term;
        if host::stream_open(sink) != 0 {
            host::print("[debug] stream_open failed\n");
            return;
        }
        host::print("[debug] kernel has no global mirror - only this terminal\n");
    }

    // Dial, then wait OUTSIDE the host call. The connect used to block in the
    // kernel until ESTABLISHED or a 10 s timeout — and this module is a fiber,
    // so those 10 s froze every other fiber on the same worker core, the WiFi
    // driver included: a failing `debug` took the link down with it. Sleeping
    // between polls leaves the core, so the driver keeps draining its card.
    let sock = host::tcp_connect(ip, port);
    let mut connected = sock >= 0;
    if connected {
        // 10 s at 20 ms — same ceiling the kernel used to enforce.
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
        host::print("[debug] tcp_connect failed (is `nc -l ");
        host::print_dec(port as u32);
        host::print("` running?)\n");
        host::stream_close(sink);
        return;
    }
    // Say who we are AFTER the sink and the socket exist — the banner above is
    // printed before either, so it only ever reached the device's own screen.
    // Which version is running, and whether it got the global mirror, is the
    // first question when output stops arriving; it has to be answerable from
    // the far end.
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
        // 0 bytes forever in CloseWait, which is why closing `nc` used to
        // leave this module running with nothing to talk to.
        // Every 16th round only: this takes the kernel's CONNECTIONS lock,
        // and under load the RX path takes the same lock tens of thousands
        // of times a second. Asking every round put a third acquisition in
        // the hot loop for an answer that changes once per session.
        round = round.wrapping_add(1);
        match if round % 16 == 0 { host::tcp_status(sock) } else { 1 } {
            1 => {}
            // -2 is the interesting one: the connection did not end, it FAILED
            // — five retransmits with no acknowledgement, i.e. the link went
            // dead under us for about six seconds. Printing the same words for
            // both hid exactly the event worth chasing.
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
                // retransmit a moment and offer the SAME bytes again, or the
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
                        // Backpressure is not failure. A MIRROR may lose lines
                        // — the sink ring already drops the oldest on overflow
                        // — but it must never disconnect over it. Closing here
                        // meant that under a saturating download the mirror
                        // shut itself down after two seconds, and announced it
                        // over the very connection it was closing: from the far
                        // end, silence with no reason given.
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

    host::tcp_close(sock);
    host::stream_close(sink);
    host::print("[debug] disconnected\n");
}
