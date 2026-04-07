//! Shade input handling — configurable mod key and compositor keybindings.
//!
//! Intercepts Mod+key combos before they reach the intent loop.
//! Mod key is configurable via `shade.mod` config (default: super).
//! Keybinds match Hyprland defaults.

use core::sync::atomic::{AtomicBool, Ordering};

/// Action buffer (single action, polled by intent loop).
static mut PENDING_ACTION: Option<ShadeAction> = None;
static HAS_ACTION: AtomicBool = AtomicBool::new(false);

/// Actions the compositor can perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadeAction {
    /// Mod+Enter: spawn new terminal window
    NewWindow,
    /// Mod+Q: close focused window
    CloseWindow,
    /// Mod+F: toggle fullscreen
    ToggleFullscreen,
    /// Mod+V: toggle floating
    ToggleFloating,
    /// Mod+1..4: switch workspace
    Workspace(u8),
    /// Mod+Shift+1..4: move window to workspace
    MoveToWorkspace(u8),
    /// Mod+Arrow: spatial focus (find window in direction)
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    /// Mod+Shift+Arrow: swap window position in tiling grid
    SwapLeft,
    SwapRight,
    SwapUp,
    SwapDown,
    /// Mod+PageUp/PageDown: scroll terminal
    ScrollUp,
    ScrollDown,
    /// Mod+L: lock
    Lock,
}

/// Check if the configured mod key is currently held (public for ESC state capture).
pub fn is_mod_active() -> bool {
    is_mod_held()
}

/// Check if the configured mod key is currently held.
/// Reads `shade.mod` config: "super" (default), "ctrl", "alt".
fn is_mod_held() -> bool {
    match crate::config::get("shade.mod").as_deref() {
        Some("ctrl") => crate::keyboard::is_ctrl_held(),
        Some("alt") => crate::keyboard::is_alt_held(),
        _ => crate::keyboard::is_super_held(), // default: super
    }
}

/// Push a pending action (public for xHCI direct dispatch).
pub fn push_action_direct(action: ShadeAction) {
    push_action(action);
}

/// Push a pending action.
fn push_action(action: ShadeAction) {
    // SAFETY: single-core, no preemption
    let p = unsafe { &mut *core::ptr::addr_of_mut!(PENDING_ACTION) };
    *p = Some(action);
    HAS_ACTION.store(true, Ordering::Release);
}

/// Poll for a pending action (called from intent loop).
pub fn poll_action() -> Option<ShadeAction> {
    if !HAS_ACTION.load(Ordering::Acquire) { return None; }
    HAS_ACTION.store(false, Ordering::Release);
    // SAFETY: single-core
    let p = unsafe { &mut *core::ptr::addr_of_mut!(PENDING_ACTION) };
    p.take()
}

/// Try to handle a key press as a shade keybinding.
/// Returns true if the key was consumed (don't pass to intent loop).
pub fn try_keybind(key: u8) -> bool {
    if !is_mod_held() { return false; }
    if !crate::shade::is_active() { return false; }

    let shift = crate::keyboard::is_shift_held();

    match key {
        b'\n' | b'\r' => {
            push_action(ShadeAction::NewWindow);
            true
        }
        b'q' | b'Q' => {
            push_action(ShadeAction::CloseWindow);
            true
        }
        b'f' | b'F' => {
            push_action(ShadeAction::ToggleFullscreen);
            true
        }
        b'v' | b'V' => {
            push_action(ShadeAction::ToggleFloating);
            true
        }
        b'l' | b'L' => {
            push_action(ShadeAction::Lock);
            true
        }
        b'1'..=b'4' => {
            let ws = key - b'0';
            if shift {
                push_action(ShadeAction::MoveToWorkspace(ws - 1));
            } else {
                push_action(ShadeAction::Workspace(ws - 1));
            }
            true
        }
        _ => false,
    }
}

/// Handle arrow key shade actions (called for ESC [ sequences).
/// Mod state already verified by caller (captured at ESC time).
/// `direction`: 'A'=up, 'B'=down, 'C'=right, 'D'=left, '5'=PgUp, '6'=PgDn
pub fn try_arrow_keybind(direction: u8) -> bool {
    if !crate::shade::is_active() { return false; }

    let shift = crate::keyboard::is_shift_held();

    match direction {
        b'A' => { push_action(if shift { ShadeAction::SwapUp } else { ShadeAction::FocusUp }); true }
        b'B' => { push_action(if shift { ShadeAction::SwapDown } else { ShadeAction::FocusDown }); true }
        b'C' => { push_action(if shift { ShadeAction::SwapRight } else { ShadeAction::FocusRight }); true }
        b'D' => { push_action(if shift { ShadeAction::SwapLeft } else { ShadeAction::FocusLeft }); true }
        b'5' => { push_action(ShadeAction::ScrollUp); true }   // PageUp
        b'6' => { push_action(ShadeAction::ScrollDown); true } // PageDown
        _ => false,
    }
}
