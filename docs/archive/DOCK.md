# DOCK.md — Auto-Hide App Dock

> **Archiv (2026-08-11).** Umgesetzt und danach überholt: der Dock läuft als
> `dock.wasm`, das Auto-Hide-Verhalten wurde in das allgemeine Panel-Primitiv
> verallgemeinert. Gültiger Vertrag ist `docs/spec/PANEL.md`; dieses Papier
> steht nur noch für die ursprüngliche Herleitung.

A bottom-edge, auto-hiding application dock for nopeekOS. A WASM app
(`dock.wasm`) like `drun`, built on the existing widget ABI, plus one
new generic compositor primitive. The dock is the fast-access surface
for pinned/favourite apps; `drun` stays the full searchable launcher.
The two are complementary, not redundant — the dock carries a grid
button that opens `drun`.

> Status: **Phase 1 spec.** Launcher-only, bottom edge, macOS-style
> auto-hide. Window-switcher behaviour (running-app indicators, focus)
> is Phase 2, gated on new `npk_window_list` / `npk_window_focus` host
> fns and intentionally out of scope here.

---

## 1. Design goals (best of all worlds)

| Source | Trait we keep |
|--------|---------------|
| macOS Dock | bottom edge, auto-hide + reveal, hover lift, transient overlay |
| GNOME Dash | pinned + (later) running apps, a grid button → full launcher |
| Windows 11 | centred icon group |

Non-goals for Phase 1: search (that's `drun`), running-window
indicators / switcher (Phase 2), fisheye neighbour-magnification
(Phase 3 polish), drag-to-reorder pins (Phase 3).

---

## 2. The one architectural decision

**Auto-hide, edge-anchoring and the reveal trigger live in the
compositor, not in the WASM app.** Only the compositor sees the global
cursor and owns z-order + screen geometry. A hidden/unfocused WASM app
receives no mouse events (events route only to the window under the
cursor), so it cannot reveal itself on a bottom-edge hit.

This respects the kernel-stays-generic rule (`memory/feedback_kernel_stays_generic.md`):
the compositor gets a generic *"edge-docked auto-hide window"*
capability — not "draw the dock". App name, icons and launch logic stay
in `dock.wasm`.

---

## 3. Tiling interaction (why auto-hide is the right pick)

A dock is a problem for a tiling WM only if it reserves screen space
(a strut), because that permanently shrinks the tiling grid. An
auto-hide overlay reserves nothing:

- The dock window is an **overlay** (`is_overlay = true`). `retile()`
  already filters `!w.is_overlay`, so the dock never enters the tiling
  grid, never shrinks it, and never triggers a reflow on show/hide.
- Mirror of the browser-Surface invariant: the browser is *always a
  tile, never fullscreen*; the dock is *never a tile, always a
  transient overlay*. Both stay in their lane.
- The dock takes **no keyboard focus** (`modal = false`, and the
  compositor does not focus it on reveal). The focused tile underneath
  stays focused; all tiling shortcuts (Mod+arrows / swap / resize) keep
  working while the dock is visible.
- The dock is **global across workspaces** (it floats above whichever
  workspace is active). Implemented via an `is_dock` flag that exempts
  it from the `active_workspace` filter in `window_at` + render.

### The one real conflict: the bottom edge

- The shadebar defaults to **Top** (`bar.rs`, `BarPosition::Top`), so
  the bottom edge is free. If a user sets `shade.bar = bottom`, dock and
  bar both want the bottom: the dock reveal anchors *above* the bar's
  reserved band (`bar.workspace_height`), the bar stays the edge anchor.
- The hot-edge must be conservative so it does not fight cursor traffic
  or a tile resize-to-bottom:
  - reveal only when the cursor sits in the **bottom `HOT_EDGE_PX`**
    (≈ 2 px) **and** no drag/resize is in progress (`comp.drag.is_none()`),
    held for a short **dwell** (`REVEAL_DWELL_TICKS`, ≈ 10 ticks @100 Hz ≈ 100 ms);
  - hide when the cursor leaves the revealed dock rect for
    `HIDE_DEBOUNCE_TICKS` (≈ 25 ticks ≈ 250 ms).

---

## 4. New ABI

```
npk_window_set_dock(w: i32, h: i32) -> i32     // RENDER cap
```

Like `npk_window_set_overlay`, but instead of centring the window the
compositor anchors it **bottom-centre**, flags it `is_overlay = true`
+ `is_dock = true`, leaves `modal = false`, and starts it **hidden**
(slid fully below the screen). The app sizes itself: width =
`icon_count * cell + padding`, height ≈ 72 px at 1× (compositor clamps
to screen). Edge is implicitly Bottom in Phase 1; the signature leaves
room to add an `edge` arg later without breaking callers.

Returns 0 on success, -1 on cap-denied / no window / bad args. Reuses
the same widget-window create/promote path as `set_overlay`.

Phase 2 (not yet): `npk_window_list(buf, max)` (ids, app names, focused
flag) + `npk_window_focus(id)` for running-app indicators + switcher.

---

## 5. Compositor state machine

New `Compositor.dock: Option<DockState>`:

```
struct DockState {
    id: WindowId,
    thickness: u32,        // dock height in px (slide distance)
    target_shown: bool,    // reveal target (set by hot-edge logic)
    offset: u32,           // 0 = fully shown, thickness = fully hidden
    dwell: u32,            // ticks cursor has held the hot-edge
    debounce: u32,         // ticks cursor has been away while shown
}
```

- `set_dock(id, w, h)` — position bottom-centre at `y = screen_h`
  (offset = thickness = h, fully hidden), `visible = false`.
- `dock_update_reveal(cursor_y)` — called from the mouse-move path.
  Bumps `dwell`/`debounce`, flips `target_shown` once thresholds pass
  (suppressed while `drag.is_some()`).
- `dock_tick()` — called each frame from the existing tick path. Eases
  `offset` toward `0` (shown) or `thickness` (hidden); sets
  `visible = offset < thickness`; window `y = screen_h - thickness + offset`;
  marks dirty + `needs_full_redraw` while moving. Reuses the ease-out
  cubic from `tick_animation`.

`window_at` and the render loop include the dock window regardless of
`active_workspace` when `is_dock`, so it floats over every workspace.

---

## 6. The `dock.wasm` app

Reuses everything `drun` already does. Factor the shared bits into an
SDK helper so both consume it:

```
nopeek_widgets::app_catalog  // enumerate sys/wasm/* modules,
                             // hydrate .npk.app_meta (icon/name/desc),
                             // append built-in intents (browser, …)
```

The app:
1. Builds the catalog, filters to the pin list from `sys/config/dock`
   (one name per line); empty/missing → fall back to the full catalog.
2. `npk_window_set_dock(width, 72)` + `npk_window_set_modal(0)`.
3. Renders a centred `Row` of icon cells (`prefab::icon_button`) + a
   trailing **launcher button** (magnifier → drun). Hover feedback in
   Phase 1 is the existing `Modifier::Hover([Background, Rounded])`
   highlight (instant re-rasterise — the compositor does NOT yet honor
   `Modifier::Scale` or animate `Transition`). The headline motion is
   the compositor-owned **slide-in reveal** (real ease-out). A true
   hover scale/lift is Phase 3 (needs rasteriser scale support).
4. Click a cell → `npk_spawn_module` (Module) / `npk_run_intent`
   (Intent); grid button → `npk_spawn_module("drun")`. Then the app
   stays resident; the compositor hides the dock when the cursor leaves.
5. Event loop mirrors `drun` (`npk_event_poll` → Action/Hover), but the
   dock does **not** close itself on launch — it is a persistent
   resident overlay, shown/hidden by the compositor.

Config: `sys/config/dock` (pins), analogous to `sys/config/launcher`.

### Launching (important)

A resident overlay must be launched via the **widget-spawn path**
(`launch_app` → `spawn_widget_app`: fresh window, no terminal, no
`APP_RUNNING`), **never** via `run <module>` — `run` captures keyboard
input (`APP_RUNNING`) and promotes the spawning shell terminal into the
dock window, so hotkeys and app launches silently break.

Wired as a generic autostart: `shade::start_autostart()` (called after
`shade::init`) reads the `autostart` config key (comma/space-separated
app names) and `launch_app`s each. The kernel stays generic — app names
live in config, not in the kernel. Enable with `set autostart dock`,
then reboot.

---

## 7. Phasing

- **Phase 1 (this doc):** `npk_window_set_dock` + compositor auto-hide;
  `app_catalog` SDK helper (drun refactored onto it); `dock.wasm` with
  pinned icons, hover-lift, grid→drun.
- **Phase 2:** `npk_window_list` + `npk_window_focus` → running-app dots
  + click-to-focus (dock becomes a unified launcher + switcher).
- **Phase 3 (polish):** fisheye magnification (needs per-frame pointer-x
  fed to the dock, or a compositor magnify pass), tooltips
  (`Widget::Tooltip` reserved slot), drag-to-reorder pins.
