# `kernel/src/shade/input.rs` @ 5e0102684

## L1-5 · `use core::sync::atomic::{AtomicBool, Ordering};`

```
//! Shade input handling — configurable mod key and compositor keybindings.
//!
//! Intercepts Mod+key combos before they reach the intent loop.
//! Mod key is configurable via `shade.mod` config (default: super).
//! Keybinds match Hyprland defaults.
```

## L9 · `static mut PENDING_ACTION: Option<ShadeAction> = None;`

```
/// Action buffer (single action, polled by intent loop).
```

## L13 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// Actions the compositor can perform.
```

## L16 · `NewWindow,`

```
/// Mod+Enter: spawn new terminal window
```

## L18 · `CloseWindow,`

```
/// Mod+Q: close focused window
```

## L20 · `ToggleFullscreen,`

```
/// Mod+F: toggle fullscreen
```

## L22 · `ToggleFloating,`

```
/// Mod+V: toggle floating
```

## L24-25 · `ToggleSplit,`

```
/// Mod+J: flip the split the focused window sits on between
/// side-by-side and stacked (Hyprland's `togglesplit`).
```

## L27 · `Workspace(u8),`

```
/// Mod+1..4: switch workspace
```

## L29 · `MoveToWorkspace(u8),`

```
/// Mod+Shift+1..4: move window to workspace
```

## L31 · `FocusLeft,`

```
/// Mod+Arrow: spatial focus (find window in direction)
```

## L36 · `SwapLeft,`

```
/// Mod+Shift+Arrow: swap window position in tiling grid
```

## L41 · `ResizeLeft,`

```
/// Mod+Ctrl+Arrow: resize focused window
```

## L46 · `ScrollUp,`

```
/// Mod+PageUp/PageDown: scroll terminal
```

## L49 · `Lock,`

```
/// Mod+L: lock
```

## L51-56 · `SpawnLauncher,`

```
/// Mod+D: spawn the configured launcher module.
///
/// The module name is read from `sys/config/launcher` at dispatch
/// time (defaults to `drun` when the file is absent). Kernel has
/// no hardcoded module name — replacing drun with a different
/// launcher is a config change, not a rebuild.
```

## L58-59 · `Copy,`

```
/// Ctrl+Shift+C: copy the focused window's selection (terminal drag
/// selection, or focused Input/TextArea) to the kernel clipboard.
```

## L61-62 · `Paste,`

```
/// Ctrl+Shift+V: paste the clipboard into the focused window (terminal
/// input line, or focused Input/TextArea).
```

## L66 · `pub fn is_mod_active() -> bool {`

```
/// Check if the configured mod key is currently held (public for ESC state capture).
```

## L71-72 · `fn is_mod_held() -> bool {`

```
/// Check if the configured mod key is currently held.
/// Reads `shade.mod` config: "super" (default), "ctrl", "alt".
```

## L77 · `_ => crate::keyboard::is_super_held(), // default: super`

```
// default: super
```

## L81 · `pub fn push_action_direct(action: ShadeAction) {`

```
/// Push a pending action (public for xHCI direct dispatch).
```

## L86-90 · `pub fn push_workspace_key(n: u8, shift: bool) {`

```
/// Workspace switch / move for digit key `n` (1-based), pushed by the
/// keyboard drivers from the RAW key — a keybinding table matching on
/// characters can never see Mod+Shift+N, because the layout has long turned
/// "1" into "+" (de_CH) or "!" (us) by then. Mod+Shift+4 on de_CH produced
/// no character at all (ç), so "move to workspace" was unreachable.
```

## L98 · `fn push_action(action: ShadeAction) {`

```
/// Push a pending action.
```

## L100 · `let p = unsafe { &mut *core::ptr::addr_of_mut!(PENDING_ACTION) };`

```
// SAFETY: single-core, no preemption
```

## L106 · `pub fn poll_action() -> Option<ShadeAction> {`

```
/// Poll for a pending action (called from intent loop).
```

## L110 · `let p = unsafe { &mut *core::ptr::addr_of_mut!(PENDING_ACTION) };`

```
// SAFETY: single-core
```

## L115-116 · `pub fn try_keybind(key: u8) -> bool {`

```
/// Try to handle a key press as a shade keybinding (legacy u8 path).
/// Returns true if the key was consumed (don't pass to intent loop).
```

## L141 · `pub fn try_arrow_keybind(direction: u8) -> bool {`

```
/// Handle arrow key shade actions (legacy, called for ESC [ sequences).
```

## L157-159 · `pub fn try_keybind_event(event: &crate::input::KeyEvent) -> bool {`

```
/// Try to handle a KeyEvent as a shade keybinding.
/// Unified handler — no separate arrow function needed.
/// Returns true if consumed.
```

