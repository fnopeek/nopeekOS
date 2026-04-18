//! debug — reverse debug shell (WASM module)
//!
//! Mirrors the window's terminal over TCP to a `nc -l <port>` listener
//! on the developer's machine. Dials out (reverse-shell style), no auth,
//! no crypto — feature is temporary, will be replaced by real SSH later.
//!
//! Usage:  run debug <ip> <port>
//! On laptop:  nc -l 22222

#![no_std]

mod host;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[debug] reverse-mirror v0.1\n");

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

    // Open stream sink first so no output from the connect attempt is lost.
    if host::stream_open(my_term) != 0 {
        host::print("[debug] stream_open failed\n");
        return;
    }

    let sock = host::tcp_connect(ip, port);
    if sock < 0 {
        host::print("[debug] tcp_connect failed (is `nc -l ");
        host::print_dec(port as u32);
        host::print("` running?)\n");
        host::stream_close(my_term);
        return;
    }
    host::print("[debug] connected\n");

    // Relay loop. Poll both directions with a short sleep to yield the core.
    let mut tx_buf = [0u8; 1024];
    let mut rx_buf = [0u8; 256];
    let mut idle_rounds: u32 = 0;

    loop {
        let mut did_work = false;

        // Terminal output → TCP
        let n = host::stream_read(my_term, &mut tx_buf);
        if n > 0 {
            if host::tcp_send(sock, &tx_buf[..n as usize]) != 0 {
                host::print("[debug] tcp_send failed — closing\n");
                break;
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
    host::stream_close(my_term);
    host::print("[debug] disconnected\n");
}
