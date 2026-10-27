# PANEL.md: the panel, status items and popups

A panel is a WASM widget app whose window the compositor keeps along the
bottom screen edge; the compositor owns the geometry, the app the content.
The shipped panel is `deck` (`tools/wasm/deck`), one bottom strip. There
is at most one panel.

## Becoming a panel

`npk_window_set_panel(edge, behavior, w, h) -> i32` (RENDER + SHELL).
The only accepted pair is `edge = 0` (bottom), `behavior = 1` (strut).
`deck` calls it once at start with that pair and `h = 44` logical px.

- The caller's widget window is created (or its terminal promoted)
  without taking focus.
- `w` and `h` are logical px, converted to physical; `h` is clamped to a
  quarter of the screen height and raised to at least 22 px. `w` is
  ignored: the window spans the screen width minus `shade.bar_margin` on
  each side.
- The window sits `shade.bar_margin` above the bottom edge. The tiling
  area gives up `margin + h` at the bottom (`bottom_band()`), plus the
  usual `shade.gaps`.
- A panel is never focused (`focus_window` refuses it), never modal, is
  excluded from `npk_window_titles`, Mod+Q and workspace moves, and follows
  the active workspace on every switch, staying visible.
- While a window is fullscreen the panel is not drawn.
- When the panel window closes, the band is released. On a UI scale
  change the compositor rescales the panel height.
- Drawing: a scene rooted in a `Widget::Stack` (as `deck`'s bar and
  popups are) is cleared transparent and its backgrounds are filled at
  the panel opacity. Panels and their popups use the dark glass palette.

-1 when the right is missing, a size is not positive, or the pair is not
`(0, 1)`.

## deck layout

Left to right inside one rounded card (`NodeId` 100000):

1. **Launcher button**: opens the launcher popup (see below).
2. **Workspaces**: one cell per workspace (count and active index from
   `npk_bar_state`). The active one is filled with the accent, empty ones
   are faint. A click calls `npk_workspace_switch(n)`.
3. **Window list**: the windows of the active workspace from
   `npk_window_titles`, ordered by window id. A button shows the app icon
   (from the app catalog) and the caption; the focused window has an
   accent line underneath. A click calls `npk_window_activate(id)`.
4. **Status items** (place 0 only), then a **screenshot** button: click
   launches `snap region`, right click `snap full`.
5. **Clock**: `HH:MM` from `npk_bar_state` over a date line ("Sun 18
   Oct") computed from `npk_unix_time` and the offset implied by the local
   `HH:MM`. Strings follow the system locale (English, German).

When the window list does not fit, `plan_tasks` gives way in this order:
buttons narrow from 200 px down to 100 px with clipped captions; the app
with the most windows is folded into a group button with a count,
repeatedly; icons only; then a "+N" button holding the rest. The focused
window keeps a place. The budget is the screen width minus fixed
estimates for the other cells. A group or "+N" toggles a popup listing its
windows, which closes when all of them are gone.

**Launcher popup** (anchored at the card, 340 px wide): a search field
(focused through `Modifier::FocusRequest`) over up to 8 rows of the app
catalog (modules with app metadata except `deck`, `drun`, `pick`,
`volume`, plus intent entries), sorted by display name, filtered by
substring of display or launch name. Up/Down and pointer hover move the
selection, Enter or a click launches (`spawn_module` or `run_intent`) and
closes. Footer: the user name (last component of the home directory) and
a restart button calling `npk_restart()` and a power button calling
`npk_power()`; either closes the popup. Esc closes the popup.

**Main loop**: drain events; re-read `npk_bar_state`, `npk_window_titles`
and `npk_status_list`; commit the bar, then the popup, only on a change;
`npk_wait(INPUT | STATE, ms to next minute)`. `STATE` (32, RENDER) wakes
on any notify topic (windows, battery, volume, config, status).

## Status items

`kernel/src/status.rs`. An item is `owner, key, icon: u16, value: i32,
state, place, text, opens`.

- `owner` is the publishing module's name, set by the kernel. At most 4
  items per owner, 32 in total. `key` is 1..16 bytes, `text` at most 48,
  `opens` at most 32 bytes and without `/`.
- `state`: 0 normal, 1 attention, 2 off (clamped). `place`: 0 bar,
  1 quick (clamped).
- Items go when the publishing fiber ends. Changes notify `TOPIC_STATUS`.

The kernel's own items are not stored but built on each read, with owner
`kernel`:

| key | icon | text | opens |
|---|---|---|---|
| `net` | WLAN or wired, off when the link is down | none | `link` |
| `volume` | speaker by level | `N%` | `volume` |
| `battery` | by charge and status; attention at 10 % or less while discharging | `N%` | none |

`net` is the primary interface (else the first). The `wifid` item `wlan`
replaces it, unless a wired primary has a link: then the `wifid` item is
left out. `battery` appears only when a battery is known.

Order: network, volume, battery, then module items by owner and key.
Wire format of `npk_status_list`: `count: u16 LE`, then per item
`owner`, `key` (u8 length + bytes), `icon: u16 LE`, `value: i32 LE`,
`state: u8`, `place: u8`, `text`, `opens` (u8 length + bytes).

`deck` draws each place-0 item as its icon, tinted by state (attention =
danger, off = faint), with its text if it has one. An item with a
non-empty `opens` is clickable: `npk_launch_at(opens, key, node)` with the
item's node as anchor. A right click on the kernel's volume item toggles
mute (restoring the previous level, at least 10 %).

**Launching a tool from an item.**

`npk_launch_at` resolves `anchor` in the caller's last layout, then does
`npk_launch(app, arg)`. On success it remembers the anchor rectangle for
that module name for 5 s. The launched app's first
`npk_window_set_overlay` within that time is placed like a popup (below
the anchor 8 logical px away, flipped above when it does not fit, slid to
stay on screen) and made light-dismiss: a press outside closes the
window. `link` and `volume` are opened this way.

## Popups

`npk_popup(popup, anchor, gap, ptr, len) -> i32` (RENDER) opens or updates
popup `popup` (0..7) of the caller's widget window.

- `ptr..ptr+len` is a wire-encoded scene tree (at most 16 MiB). The popup
  is exactly its intrinsic size, clamped to the screen.
- `anchor` is a `NodeId` in the app's own last committed tree; commit
  the main tree first. `gap` is 0..256 logical px.
- Placement: below the anchor at `gap`; above it when it does not fit
  below; else bottom-aligned to the screen. `x` is the anchor's left edge,
  slid to stay on screen. At the bottom panel this always flips above.
- Opening a new popup needs the user: the parent window or one of its
  popups has the focus, or the last press or key (within 1 s) went to the
  parent; otherwise `npk_popup` returns -1. A panel, which is never
  focused, opens popups only in answer to a click on it.
- A new popup takes the keyboard; the focus before the first popup is
  remembered. Its events go to the parent window's queue. Popups are
  drawn above all panels and windows. Calling `npk_popup` again for an
  open slot re-places and re-commits it.

- `npk_popup_close(popup)` and closing the parent window: no `PopupDone`.
- The compositor sends `Event::PopupDone { popup }` to the parent when it
  closes a popup itself: a press (left or right) outside every popup
  (closes all, consumed); focus moving to a window that is not a popup;
  a workspace switch; a UI scale change.
- After a close, the keyboard goes to another open popup, else back to the
  remembered window if it is still visible on the active workspace, else
  to the active workspace's top window.

`deck` uses slot 0 only; on `PopupDone` it just forgets its menu state.

## Host functions used by the panel

| Function | Right | Returns |
|---|---|---|
| `npk_window_set_panel(edge, behavior, w, h)` | RENDER + SHELL | 0 / -1; only `(0, 1)`, the bottom strut |
| `npk_bar_state(buf, max)` | RENDER | bytes; `"HH:MM\n<ws_count>\n<ws_active>\n<title>"`, local time from `timezone`, truncated to `max` |
| `npk_window_titles(buf, max)` | RENDER | bytes; lines `<flags>\t<workspace>\t<module>`, plus `\t<id>\t<caption>` for SHELL; flags 1 focused, 2 on the active workspace |
| `npk_window_activate(id)` | RENDER + SHELL | 0 / -1; switches to the window's workspace and focuses it; refuses panels and overlays |
| `npk_workspace_switch(n)` | RENDER | 0 / -1; `n` in 0..255, ignored past the workspace count |
| `npk_popup(...)`, `npk_popup_close(popup)` | RENDER | 0 / -1 |
| `npk_status_set(key, icon, value, flags, text, opens)` | none | 0 / -1; `flags = state \| place << 8` |
| `npk_status_clear(key)` | none | 0 |
| `npk_status_list(buf, max)` | RENDER | bytes, the needed size when larger than `max`, -1 when refused |
| `npk_launch_at(app, arg, anchor)` | EXECUTE | 0 / -1 |
| `npk_power()`, `npk_restart()` | RENDER + SHELL | 0 / -1, returns at once. Every app window is asked to close as the user's close gesture would (a guarded one gets `CloseRequest`, a microVM window asks its guest to end and the home image is saved); when none is left and no guest runs (at most 60 s for the guest), npkFS syncs and the machine powers off or restarts. An app that answers with `npk_window_close_cancel` cancels it. The `halt` and `reboot` intents take the same path |

`deck` declares `READ | EXEC | RENDER` and the extended `SHELL` right.
Only `npk_status_set` / `npk_status_clear` (`npk_sys::status_set`,
`status_clear`) and `npk_window_titles` (`host::window_titles`) have SDK
wrappers; `deck` declares the other imports itself.

## Configuration

| Key | Effect |
|---|---|
| `autostart` | modules started at boot, space or comma separated, 400 ms apart; the setup seed is `deck acpi audio_hda` |
| `shade.bar_margin` | gap between the panel and the screen edge, logical px (default 6) |
| `shade.chrome_opacity` | panel opacity 0..255 (default derived from the wallpaper); setting it clears `shade.bar_opacity` |
| `shade.bar_opacity` | overrides the opacity of the panel |
| `timezone` | zone of the clock in `npk_bar_state` (a zone name such as `Europe/Zurich`, a POSIX TZ string, or a fixed offset), daylight saving included |

## Present in the compositor, unused

No shipped module uses these:

- The `title` line of `npk_bar_state` and the `value` field of status
  items: provided, but `deck` does not show them.
- Status items with place 1 (quick): accepted, not drawn by `deck`.
