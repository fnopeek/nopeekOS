//! npk-shell: placeholder for future SSH-compatible remote access.
//! The old custom protocol was removed — will be replaced with SSH compatibility.

/// Start listener — no-op (shell removed, SSH planned).
pub fn start_listener() {}

/// Check and serve — no-op.
pub fn check_and_serve(_vault: &spin::Mutex<crate::capability::Vault>, _session: crate::capability::CapId) {}

/// Serve one connection — no-op.
pub fn serve_one(_vault: &spin::Mutex<crate::capability::Vault>, _session: crate::capability::CapId) {
    crate::kprintln!("[npk] npk-shell removed — SSH-compatible remote access planned");
}
