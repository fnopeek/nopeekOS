# POPUP.md — a window of its own for a menu

Status: **built** (2026-10-20, Kernel 0.653.0, deck 0.3.0). Replaces
`set_panel(Bottom, Raised)`.

## Why

A menu that opens from a panel had to live inside the panel's window. The
deck raised its window to full width × 520 px for a 340 px menu: some
3500 × 470 transparent pixels laid out, rasterised and composed on every
commit, in a buffer of 8 MB that the GPU module bound anew each time
(each commit allocates a new `PixelBuffer`). On the NUC the binding failed
and the compositor fell back to the CPU; the pointer stuttered. The dock's
right-click menu does the same with the full screen height.

A menu is a window: as large as its content, placed next to what opened
it, gone when the user looks elsewhere. That is Wayland's `xdg_popup`, and
this follows it.

## The model (after `xdg_popup` + `xdg_positioner`)

- **Parent.** A popup belongs to the app's widget window. It lives and dies
  with it: closing the parent, or the app's end, closes its popups.
- **Anchor.** A `NodeId` in the parent's tree. The compositor already
  knows every anchor's rectangle from the layout (`Popover` uses the same
  map), so the app names a node, not coordinates it cannot know.
- **Size.** The intrinsic size of the popup's tree, measured by the
  compositor at each commit. The window is exactly that large.
- **Position** (positioner: anchor edge bottom-left, gravity down, then
  `flip_y` and `slide_x`): below the anchor at `gap` px; if it does not fit
  on screen, above it at `gap`; slid horizontally to stay on screen. It
  follows each commit, so a list that grows or shrinks stays attached.
- **Grab.** A popup takes the keyboard (`xdg_popup.grab`). A press outside
  every popup of the app dismisses them and is consumed; so does the focus
  moving elsewhere (Mod+number, a new window). The app learns it through
  `Event::PopupDone { popup }` (`xdg_popup.popup_done`). Esc is the app's:
  it receives the key and closes the popup itself, as a Wayland client does.
  On close the keyboard goes back to the window that had it.
- **Events.** A popup has no queue of its own: what happens in it (actions,
  keys, input changes) arrives in the parent's queue. Action ids are the
  app's; it keeps them distinct.
- **Look.** Drawn like a panel: transparent outside the app's own card, so
  rounded corners stay round, in the panel's palette when the parent is a
  panel.

## ABI

```
npk_popup(popup, anchor_node, gap, ptr, len) -> i32   // RENDER
npk_popup_close(popup) -> i32                         // RENDER
Event::PopupDone { popup: u32 }                       // appended
```

`popup` is the app's own number (0..7). `npk_popup` opens the popup on
first use and commits a new tree on every later one; the tree is the same
wire format as `npk_scene_commit`. -1: no parent window, no such anchor
(not laid out yet), or the tree does not decode.

## Gone with it

`set_panel(Bottom, Raised)` and everything that served it: the raised
height, the keyboard exception for a raised panel, `Event::Focus(false)` to
a panel. The bottom strut stays.

## Later

- The dock's right-click menu and add-app list on popups; its window then
  stays 50 px tall instead of growing to the screen.
- Nested popups (a submenu): the model allows it, the first version does not
  need it.
