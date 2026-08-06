# PANEL.md — Customizable Panel System (Bar + Dock)

Generalise the dock into a **panel** primitive: edge-docked WASM widget
surfaces, rendered through the widget ABI (SDF AA, theme tokens, the
translucent-tray + crisp-icon blit), customizable via config and
replaceable because they're plain WASM apps.

> Status: **spec.** Approved design (2026-05-26): unified `set_panel`,
> config-driven segments, WASM-replaceable. The top bar moves out of the
> kernel (`bar.rs` native render) into `bar.wasm`, analogous to `dock.wasm`.

---

## 1. Why

- **Sharper edges** — the widget rasteriser's SDF AA beats the native
  `fill_rounded_rect_*` the bar uses today.
- **No UI transparency/AA code in the kernel** — the compositor only owns
  the panel *geometry* (edge, strut/overlay, reveal); the app owns content.
  Respects `memory/feedback_kernel_stays_generic.md`.
- **One render path** for bar + dock + drun.
- **Customizable + replaceable** — config-driven content, and a power user
  can ship their own `bar.wasm` / `dock.wasm`.

## 2. The panel primitive

Replace `npk_window_set_dock(w,h)` with:

```
npk_window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) -> i32   // RENDER cap
```

- `edge`: 0=Bottom, 1=Top (2=Left, 3=Right reserved for later).
- `behavior`: 0=AutoHideOverlay (dock — slides in on edge-hover, no strut),
  1=Strut (bar — always visible, reserves its band from the tiling area).
- `set_dock(w,h)` becomes a thin wrapper → `set_panel(Bottom, AutoHide, w,h)`.

Compositor: `DockState` → `PanelState { id, edge, behavior, thickness, gap,
offset, target_shown, dwell, debounce }`. Bottom+AutoHide keeps today's
reveal/slide/handle. Top+Strut: positioned into the bar band, `is_overlay +
is_panel`, never modal/focused, global across workspaces, **rendered with
the same translucent-tray + crisp-icon blit as the dock** (generalise the
`is_dock` branch to `is_dock || is_bar`).

**Strut ownership (interim):** keep the existing `ShadeBar` geometry
(`workspace_y` / `workspace_height` / `pill_top`) as the strut source so the
tiling band stays exactly as today — zero tiling refactor. `set_panel(Top,
Strut)` positions the bar window into that band and sets `bar.height` from
the requested `h`. Native `ShadeBar` pill-drawing is removed; only its
geometry remains.

## 3. Live state + actions (host fns)

The bar needs kernel state a WASM app can't compute:

```
npk_bar_state(buf, max) -> i32     // fills: "HH:MM\n<ws_count>\n<ws_active>\n<title>"
npk_workspace_switch(n) -> i32     // comp.switch_workspace(n)
npk_power() -> i32                 // acpi::power_off path
```

All RENDER-gated (bar is a trusted bundled app). `npk_bar_state` sources:
clock = `rtc::read_unix_time`, ws_count = `ShadeBar.workspace_count`,
ws_active = `comp.active_workspace`, title = focused window's `title`.

**Live update:** the bar polls `npk_bar_state` (~1 Hz) and re-commits its
tree only when the string changed (clock minute / workspace / title). No
kernel push needed.

## 4. The render-site finding (why this is bigger than the dock)

The native bar is drawn at **4 sites** in `compositor.rs` — full render
(769), partial-chrome (1128), and the LAYER_CHROME path (1687, 1729). The
dock was a pure overlay with one draw. Migration must route all of these to
"render the bar widget window (if present) else native fallback", and the
translucent bar must **restore its band from the wallpaper layer before
blitting** (frequent clock repaints would otherwise stack translucency).
Keeping a native fallback when no `bar.wasm` window exists avoids a
clock-less regression if the app fails to load.

## 5. Customization (config-driven)

- `sys/config/bar` — ordered segments, e.g.
  ```
  left: workspaces title
  center: clock
  right: tray power
  ```
  Built-in segment widgets (Phase 1): `workspaces`, `title`, `clock`,
  `tray` (status icons), `power`. Unknown names skipped. Missing config →
  built-in default layout above.

  Same file, sizing (re-read every ~3 s, so editing it is edit-and-look):
  ```
  font: 15    # text px, 9..18 (default 15)
  icon: 18    # icon px, 12..22 (default 18)
  ```
  Both are capped to what the 24 px content band holds — turning them up
  never makes the bar taller.
- `sys/config/dock` — pins (already shipped).
- Panel translucency is compositor-side: `set shade.chrome_opacity <0-255>`
  is the master — it moves both panels and **clears** the per-panel keys.
  `shade.bar_opacity` / `shade.dock_opacity` then override one of them
  again (unset = inherit the shared value).
- Placement/behaviour as config keys later: `bar.edge`, `bar.height`,
  `dock.autohide`, … (read by the apps / passed to `set_panel`).
- Both apps are WASM → fully replaceable.

## 6. `bar.wasm`

`tools/wasm/bar/`, analogous to `tools/wasm/dock/`. `_start`: read
`sys/config/bar`, `set_panel(Top, Strut, w, h)`, loop { poll `npk_bar_state`
→ re-commit on change; poll events → `workspace_switch` / `power` clicks }.
Tree: `Row` of the configured segments with flex `Spacer`s distributing
left/center/right. Bundled into the installer; autostart seed becomes
`dock bar`.

## 7. Phasing

- **P1 (this spec):** unified `set_panel` (dock = wrapper); `is_bar` strut
  window reusing `ShadeBar` geometry + the dock blit; `npk_bar_state` /
  `workspace_switch` / `power`; `bar.wasm` with config segments + native
  fallback; bundle + autostart.
- **P2:** placement config (`bar.edge`/`height`), left/right panels,
  per-element styling.
- **P3:** user-defined custom segment widgets.
