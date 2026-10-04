//! npk-shell: placeholder for SSH-compatible remote access. Not implemented.

/// Start listener — no-op (shell removed, SSH planned).
pub fn start_listener() {}

/// Check and serve — no-op.
pub fn check_and_serve(_vault: &spin::Mutex<crate::capability::Vault>, _session: crate::capability::CapId) {}

/// Serve one connection — no-op.
pub fn serve_one(_vault: &spin::Mutex<crate::capability::Vault>, _session: crate::capability::CapId) {
    crate::kprintln!("[npk] npk-shell removed — SSH-compatible remote access planned");
}
