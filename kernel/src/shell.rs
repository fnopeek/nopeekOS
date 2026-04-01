//! npk-shell: Encrypted remote intent loop
//!
//! Protocol (npk-shell v1):
//!   1. Server → Client: 32 bytes X25519 public key
//!   2. Client → Server: 32 bytes X25519 public key
//!   3. Both derive shared secret → HKDF → session keys
//!   4. All messages: len(2 BE) + ChaCha20-Poly1305(data + 16-byte tag)
//!   5. Auth: server sends "PASSPHRASE?", client sends passphrase
//!   6. On success: bidirectional encrypted intent loop
//!
//! Auto-starts at boot. Checked during intent loop idle time.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::{kprintln, crypto, csprng, net::tcp};
use crate::tls::{x25519, hmac};

const SHELL_PORT: u16 = 4444;

/// Global listener handle (None = not started).
static LISTENER: Mutex<Option<usize>> = Mutex::new(None);

/// Start listening. Called once at boot after network + identity are up.
pub fn start_listener() {
    match tcp::listen(SHELL_PORT) {
        Ok(handle) => {
            *LISTENER.lock() = Some(handle);
            kprintln!("[npk-shell] Listening on port {}", SHELL_PORT);
        }
        Err(e) => kprintln!("[npk-shell] Failed to listen: {}", e),
    }
}

/// Non-blocking check: if a client connected, serve them, then re-listen.
/// Called from the intent loop during idle polling.
pub fn check_and_serve(vault: &'static Mutex<crate::capability::Vault>, session_id: crate::capability::CapId) {
    let handle = match *LISTENER.lock() {
        Some(h) => h,
        None => return,
    };

    if !tcp::is_established(handle) {
        return;
    }

    // Client connected — handle the session
    kprintln!("\n[npk-shell] Client connected.");
    handle_session(handle, vault, session_id);

    // Re-listen for next client
    match tcp::reset_to_listen(handle) {
        Ok(()) => {}
        Err(_) => {
            // Fallback: close and re-create listener
            let _ = tcp::close(handle);
            match tcp::listen(SHELL_PORT) {
                Ok(h) => { *LISTENER.lock() = Some(h); }
                Err(_) => { *LISTENER.lock() = None; }
            }
        }
    }
}

struct Session {
    tcp_handle: usize,
    send_key: [u8; 32],
    recv_key: [u8; 32],
    send_iv: [u8; 12],
    recv_iv: [u8; 12],
    send_seq: u64,
    recv_seq: u64,
}

impl Session {
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        let nonce = build_nonce(&self.send_iv, self.send_seq);
        self.send_seq += 1;
        let encrypted = crypto::aead_encrypt(&self.send_key, &nonce, data);
        let len = encrypted.len() as u16;
        let mut frame = Vec::with_capacity(2 + encrypted.len());
        frame.push((len >> 8) as u8);
        frame.push(len as u8);
        frame.extend_from_slice(&encrypted);
        tcp::send(self.tcp_handle, &frame).map_err(|_| "send failed")
    }

    fn recv(&mut self) -> Result<Vec<u8>, &'static str> {
        let mut hdr = [0u8; 2];
        recv_exact(self.tcp_handle, &mut hdr)?;
        let len = ((hdr[0] as usize) << 8) | hdr[1] as usize;
        if len == 0 || len > 65535 { return Err("invalid frame"); }
        let mut buf = alloc::vec![0u8; len];
        recv_exact(self.tcp_handle, &mut buf)?;
        let nonce = build_nonce(&self.recv_iv, self.recv_seq);
        self.recv_seq += 1;
        crypto::aead_decrypt(&self.recv_key, &nonce, &buf).ok_or("decrypt failed")
    }

    fn send_str(&mut self, s: &str) -> Result<(), &'static str> {
        self.send(s.as_bytes())
    }

    fn recv_line(&mut self) -> Result<String, &'static str> {
        let data = self.recv()?;
        let s = core::str::from_utf8(&data).map_err(|_| "invalid utf-8")?;
        Ok(String::from(s.trim()))
    }
}

fn build_nonce(iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut nonce = *iv;
    let seq_bytes = seq.to_be_bytes();
    for i in 0..8 { nonce[4 + i] ^= seq_bytes[i]; }
    nonce
}

fn derive_keys(shared: &[u8; 32]) -> ([u8; 32], [u8; 32], [u8; 12], [u8; 12]) {
    let salt = b"npk-shell-v1\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
    let prk = hmac::hkdf_extract(salt, shared);
    let mut s2c_key = [0u8; 32];
    let mut c2s_key = [0u8; 32];
    let mut s2c_iv = [0u8; 12];
    let mut c2s_iv = [0u8; 12];
    s2c_key.copy_from_slice(&hmac::hkdf_expand_label(&prk, b"s2c key", &[], 32));
    c2s_key.copy_from_slice(&hmac::hkdf_expand_label(&prk, b"c2s key", &[], 32));
    s2c_iv.copy_from_slice(&hmac::hkdf_expand_label(&prk, b"s2c iv", &[], 12));
    c2s_iv.copy_from_slice(&hmac::hkdf_expand_label(&prk, b"c2s iv", &[], 12));
    (s2c_key, c2s_key, s2c_iv, c2s_iv)
}

fn recv_exact(handle: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    let mut filled = 0;
    let mut idle = 0u32;
    while filled < buf.len() {
        crate::net::poll();
        let n = tcp::recv_blocking(handle, &mut buf[filled..], 100) // 1s poll
            .map_err(|_| "recv failed")?;
        if n > 0 {
            filled += n;
            idle = 0;
        } else {
            idle += 1;
            // 30 minutes idle timeout (1800 × 1s polls)
            if idle > 1800 { return Err("idle timeout"); }
        }
    }
    Ok(())
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 { diff |= a[i] ^ b[i]; }
    diff == 0
}

/// Handle a single client session (key exchange, auth, intent loop).
fn handle_session(handle: usize, vault: &'static Mutex<crate::capability::Vault>, session_id: crate::capability::CapId) {
    // X25519 key exchange
    let server_private = csprng::random_256();
    let server_public = x25519::x25519_base(&server_private);

    if tcp::send(handle, &server_public).is_err() {
        kprintln!("[npk-shell] Key exchange failed");
        let _ = tcp::close(handle);
        return;
    }

    let mut client_public = [0u8; 32];
    if recv_exact(handle, &mut client_public).is_err() {
        kprintln!("[npk-shell] Key exchange failed");
        let _ = tcp::close(handle);
        return;
    }

    let shared = x25519::x25519(&server_private, &client_public);
    let (s2c_key, c2s_key, s2c_iv, c2s_iv) = derive_keys(&shared);

    let mut sess = Session {
        tcp_handle: handle,
        send_key: s2c_key,
        recv_key: c2s_key,
        send_iv: s2c_iv,
        recv_iv: c2s_iv,
        send_seq: 0,
        recv_seq: 0,
    };

    kprintln!("[npk-shell] Encrypted (X25519 + ChaCha20-Poly1305).");

    // Authenticate
    if sess.send_str("npk-shell v1\nPASSPHRASE?").is_err() {
        let _ = tcp::close(handle);
        return;
    }

    let passphrase = match sess.recv() {
        Ok(data) => data,
        Err(_) => {
            kprintln!("[npk-shell] Auth failed.");
            let _ = tcp::close(handle);
            return;
        }
    };

    let salt = crate::npkfs::install_salt().unwrap_or([0u8; 16]);
    let test_key = crypto::derive_master_key(&passphrase, &salt);
    let authed = match crypto::get_master_key() {
        Some(mk) => constant_time_eq(&test_key, &mk),
        None => false,
    };
    drop(passphrase);

    if !authed {
        let _ = sess.send_str("DENIED");
        kprintln!("[npk-shell] Wrong passphrase.");
        let _ = tcp::close(handle);
        return;
    }

    if sess.send_str("OK").is_err() {
        let _ = tcp::close(handle);
        return;
    }

    kprintln!("[npk-shell] Authenticated. Remote session active.");

    // Intent loop
    loop {
        let user = crate::config::get("name");
        let cwd = crate::intent::get_cwd_for_shell();
        let user_str = user.as_deref().unwrap_or("npk");
        let prompt = if cwd.is_empty() {
            alloc::format!("{}@npk /> ", user_str)
        } else {
            alloc::format!("{}@npk {}> ", user_str, cwd)
        };

        if sess.send_str(&prompt).is_err() { break; }

        let input = match sess.recv_line() {
            Ok(s) => s,
            Err(_) => break,
        };

        if input.is_empty() { continue; }

        if input == "exit" || input == "quit" || input == "disconnect" {
            let _ = sess.send_str("[npk-shell] Disconnected.\n");
            break;
        }

        crate::serial::start_capture();
        crate::intent::dispatch_for_shell(&input, vault, session_id);
        let output = crate::serial::stop_capture();

        if sess.send(output.as_bytes()).is_err() { break; }
    }

    let _ = tcp::close(handle);
    kprintln!("[npk-shell] Client disconnected.");
}

#[allow(dead_code)]
const DEBUG_PORT: u16 = 4445;

#[allow(dead_code)]
/// Start a plaintext debug listener on port 4445 (pre-setup, no auth).
/// Allows remote diagnosis when framebuffer doesn't work.
/// Blocks until a client connects, then dumps boot log and disconnects.
pub fn start_debug_listener() {
    let handle = match tcp::listen(DEBUG_PORT) {
        Ok(h) => h,
        Err(e) => { kprintln!("[npk-debug] Listen failed: {}", e); return; }
    };

    kprintln!("[npk-debug] Debug shell on port {} (plaintext, waiting...)", DEBUG_PORT);

    // Wait up to 60 seconds for a debug client
    if let Err(_) = tcp::accept(handle, 6000) {
        // No client connected — continue boot normally
        let _ = tcp::close(handle);
        kprintln!("[npk-debug] No client, continuing boot.");
        return;
    }

    kprintln!("[npk-debug] Client connected.");

    // Send the entire boot log captured so far
    let boot_log = crate::serial::stop_capture();
    crate::serial::start_capture(); // restart capture for further output

    let banner = alloc::format!(
        "=== nopeekOS Debug Shell (plaintext, port {}) ===\r\n\
         === Boot log: ===\r\n{}\r\n\
         === Type commands, 'exit' to continue boot ===\r\n",
        DEBUG_PORT, boot_log
    );
    let _ = tcp::send(handle, banner.as_bytes());

    // Simple line-based command loop (plaintext)
    let mut buf = [0u8; 512];
    loop {
        let _ = tcp::send(handle, b"debug> ");

        // Read a line
        let mut line = alloc::vec::Vec::new();
        loop {
            crate::net::poll();
            match tcp::recv_blocking(handle, &mut buf, 100) {
                Ok(0) => {}
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            break;
                        }
                        line.push(b);
                    }
                    if buf[..n].contains(&b'\n') || buf[..n].contains(&b'\r') {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tcp::close(handle);
                    return;
                }
            }
        }

        let input = core::str::from_utf8(&line).unwrap_or("").trim();
        if input.is_empty() { continue; }

        if input == "exit" || input == "quit" || input == "continue" {
            let _ = tcp::send(handle, b"Continuing boot...\r\n");
            let _ = tcp::close(handle);
            kprintln!("[npk-debug] Client disconnected, resuming boot.");
            return;
        }

        // Execute as diagnostic: show specific info
        let response = match input {
            "fb" | "framebuffer" => {
                if crate::framebuffer::is_available() {
                    alloc::format!("Framebuffer: active\r\n")
                } else {
                    alloc::format!("Framebuffer: NOT available\r\n")
                }
            }
            "log" => {
                let log = crate::serial::stop_capture();
                crate::serial::start_capture();
                alloc::format!("{}\r\n", log)
            }
            _ => alloc::format!("Unknown: '{}'. Commands: fb, log, exit\r\n", input),
        };
        let _ = tcp::send(handle, response.as_bytes());
    }
}

/// Manual start from intent loop (kept for backwards compat).
pub fn serve_one(vault: &'static spin::Mutex<crate::capability::Vault>, session_id: crate::capability::CapId) {
    let handle = match tcp::listen(SHELL_PORT) {
        Ok(h) => h,
        Err(e) => { kprintln!("[npk-shell] Listen failed: {}", e); return; }
    };

    kprintln!("[npk-shell] Listening on port {}...", SHELL_PORT);

    if let Err(e) = tcp::accept(handle, 0) {
        kprintln!("[npk-shell] Accept failed: {}", e);
        let _ = tcp::close(handle);
        return;
    }

    handle_session(handle, vault, session_id);
}
