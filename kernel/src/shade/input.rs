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
    /// Mod+Ctrl+Arrow: resize focused window
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    /// Mod+PageUp/PageDown: scroll terminal
    ScrollUp,
    ScrollDown,
    /// Mod+L: lock
    Lock,
    /// Mod+D: spawn the configured launcher module.
    ///
    /// The module name is read from `sys/config/launcher` at dispatch
    /// time (defaults to `drun` when the file is absent). Kernel has
    /// no hardcoded module name — replacing drun with a different
    /// launcher is a config change, not a rebuild.
    SpawnLauncher,
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

/// Try to handle a key press as a shade keybinding (legacy u8 path).
/// Returns true if the key was consumed (don't pass to intent loop).
pub fn try_keybind(key: u8) -> bool {
    if !is_mod_held() { return false; }
    if !crate::shade::is_active() { return false; }

    let shift = crate::keyboard::is_shift_held();

    match key {
        b'\n' | b'\r' => { push_action(ShadeAction::NewWindow); true }
        b'q' | b'Q' => { push_action(ShadeAction::CloseWindow); true }
        b'f' | b'F' => { push_action(ShadeAction::ToggleFullscreen); true }
        b'v' | b'V' => { push_action(ShadeAction::ToggleFloating); true }
        b'l' | b'L' => { push_action(ShadeAction::Lock); true }
        b'd' | b'D' => { push_action(ShadeAction::SpawnLauncher); true }
        b'1'..=b'4' => {
            let ws = key - b'0';
            if shift { push_action(ShadeAction::MoveToWorkspace(ws - 1)); }
            else { push_action(ShadeAction::Workspace(ws - 1)); }
            true
        }
        _ => false,
    }
}

/// Handle arrow key shade actions (legacy, called for ESC [ sequences).
pub fn try_arrow_keybind(direction: u8) -> bool {
    if !crate::shade::is_active() { return false; }
    let shift = crate::keyboard::is_shift_held();
    let ctrl = crate::keyboard::is_ctrl_held();
    match direction {
        b'A' => { push_action(if ctrl { ShadeAction::ResizeUp } else if shift { ShadeAction::SwapUp } else { ShadeAction::FocusUp }); true }
        b'B' => { push_action(if ctrl { ShadeAction::ResizeDown } else if shift { ShadeAction::SwapDown } else { ShadeAction::FocusDown }); true }
        b'C' => { push_action(if ctrl { ShadeAction::ResizeRight } else if shift { ShadeAction::SwapRight } else { ShadeAction::FocusRight }); true }
        b'D' => { push_action(if ctrl { ShadeAction::ResizeLeft } else if shift { ShadeAction::SwapLeft } else { ShadeAction::FocusLeft }); true }
        b'5' => { push_action(ShadeAction::ScrollUp); true }
        b'6' => { push_action(ShadeAction::ScrollDown); true }
        _ => false,
    }
}

/// Try to handle a KeyEvent as a shade keybinding.
/// Unified handler — no separate arrow function needed.
/// Returns true if consumed.
pub fn try_keybind_event(event: &crate::input::KeyEvent) -> bool {
    use crate::input::KeyCode;
    if !event.modifiers.super_key { return false; }
    if !crate::shade::is_active() { return false; }

    let shift = event.modifiers.shift;
    let ctrl = event.modifiers.ctrl;

    match event.key {
        KeyCode::Enter => { push_action(ShadeAction::NewWindow); true }
        KeyCode::Char(b'q') | KeyCode::Char(b'Q') => { push_action(ShadeAction::CloseWindow); true }
        KeyCode::Char(b'f') | KeyCode::Char(b'F') => { push_action(ShadeAction::ToggleFullscreen); true }
        KeyCode::Char(b'v') | KeyCode::Char(b'V') => { push_action(ShadeAction::ToggleFloating); true }
        KeyCode::Char(b'l') | KeyCode::Char(b'L') => { push_action(ShadeAction::Lock); true }
        KeyCode::Char(b'd') | KeyCode::Char(b'D') => { push_action(ShadeAction::SpawnLauncher); true }
        KeyCode::Char(b'1'..=b'4') => {
            if let KeyCode::Char(c) = event.key {
                let ws = c - b'0';
                if shift { push_action(ShadeAction::MoveToWorkspace(ws - 1)); }
                else { push_action(ShadeAction::Workspace(ws - 1)); }
            }
            true
        }
        KeyCode::Up => { push_action(if ctrl { ShadeAction::ResizeUp } else if shift { ShadeAction::SwapUp } else { ShadeAction::FocusUp }); true }
        KeyCode::Down => { push_action(if ctrl { ShadeAction::ResizeDown } else if shift { ShadeAction::SwapDown } else { ShadeAction::FocusDown }); true }
        KeyCode::Right => { push_action(if ctrl { ShadeAction::ResizeRight } else if shift { ShadeAction::SwapRight } else { ShadeAction::FocusRight }); true }
        KeyCode::Left => { push_action(if ctrl { ShadeAction::ResizeLeft } else if shift { ShadeAction::SwapLeft } else { ShadeAction::FocusLeft }); true }
        KeyCode::PageUp => { push_action(ShadeAction::ScrollUp); true }
        KeyCode::PageDown => { push_action(ShadeAction::ScrollDown); true }
        _ => false,
    }
}
