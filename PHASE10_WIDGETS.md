# Phase 10 — Widget API & GUI Apps

**Goal:** Apps describe **what** to render (declarative widget tree). The Shade compositor owns **how** (layout, rasterization, GPU compositing, animation, theming). Apps never touch pixels for normal UI; a single tightly-scoped `Canvas` escape hatch exists for image/chart use cases.

**Sweet spot between immediate-mode (App calls `draw_rect`) and full retained-mode (App holds scene-graph handles):** App calls `render()` whenever its state changes, builds a fresh tree as plain data, commits in one host call. Compositor diffs against previous tree and only re-rasterizes changed sub-trees.

Inspired by SwiftUI / Slint / Compose — stripped down, capability-gated, WASM-native.

---

## Why this fits nopeekOS

| Principle | Fit |
|---|---|
| Capabilities, not Permissions | Tree commit + canvas commit are separate cap-gated host calls |
| Intents, not Commands | App declares intent (`List{items, selected}`), compositor executes |
| Sandboxed | App has zero access to GPU, framebuffer, fonts |
| No legacy | No GDI handles, no X11 protocol, no CSS box model |
| Greenfield | Layer-tree is data, not API-call stream — auditable, replayable, deterministic |
| Content-addressed | Font files live in npkFS, BLAKE3-verified |

---

## What stays untouched

- Shade compositor (tiling, swap-anim, mouse) — `kernel/src/shade/`
- Window management, keybindings, shadebar
- Loop / intent dispatcher (`kernel/src/intent/`)
- Existing rendering primitives (`gui/render.rs`) — reused, target changes from shadow to layer texture
- Spleen bitmap font (`gui/font.rs`) — kept for terminal contexts (`loop`, `top`, REPLs); bitmap mono looks correct in terminals
- BCS blitter, GPU HAL, GGTT layout (extended, not replaced)
- Wallpaper-driven theme extraction (`gui/color.rs`)

The widget system is **additive** — only new code, no rewrites.

---

## App-side example: File browser

```rust
// tools/wasm/files/src/lib.rs
use nopeek_widgets::*;

struct Files {
    cwd: String,
    entries: Vec<Entry>,
    cursor: usize,
}

impl App for Files {
    fn render(&self) -> Widget {
        Column::new()
            .child(Toolbar::new()
                .button(Icon::ArrowLeft, "back")
                .button(Icon::ArrowUp, "up")
                .breadcrumb(&self.cwd))
            .child(Row::new()
                .child(Sidebar::new()
                    .item(Icon::Home, "Home")
                    .item(Icon::Folder, "Documents")
                    .item(Icon::Download, "Downloads"))
                .child(List::new(&self.entries)
                    .selected(self.cursor)
                    .row(|e| Row::new()
                        .child(Icon::for_entry(e))
                        .child(Text::new(&e.name))
                        .child(Text::new(&e.size).muted()))))
            .child(StatusBar::text(format!("{} items", self.entries.len())))
    }

    fn handle(&mut self, ev: Event) -> Action {
        match ev {
            Event::Key(KeyCode::Down) => { self.cursor += 1; Action::Rerender }
            Event::Key(KeyCode::Enter) => self.open_selected(),
            _ => Action::Idle,
        }
    }
}
```

App returns `Action::Rerender` → SDK calls `render()` → serializes tree → `npk_scene_commit(bytes, cap)`.
No animation code. No pixel code. No font code. No theme code.

---

## Data flow

```
WASM App
   │
   │  Widget tree (Rust struct)
   ▼
nopeek_widgets SDK (postcard serialize, version-prefixed)
   │
   │  ~few KB bytes
   ▼
npk_scene_commit(ptr, len, render_cap)   ◄─ host fn, capability-gated
   │
   ▼
Compositor
   ├─ Read version byte → reject if unsupported
   ├─ Deserialize tree (postcard)
   ├─ Layout pass (flexbox-lite) → assigns x/y/w/h to every node
   ├─ Diff against previous tree (structural ID + content hash)
   ├─ For each changed sub-tree:
   │     └─ Rasterize into layer texture in GGTT slab (CPU; fontdue + gui/render.rs)
   │
   ▼
GPU (BCS XY_FAST_COPY_BLT)
   └─ Blit layer textures → window region in framebuffer
```

**Key property:** App-side allocations free after `commit`. Compositor owns everything that survives the call.

---

## Widget set v1

Minimal, composable, no leaf escape hatches except the strictly-scoped `Canvas`.

**Containers:**
- `Column { children, spacing, align }`
- `Row { children, spacing, align }`
- `Stack { children }` — z-order overlay
- `Scroll { child, axis }`

**Leaves:**
- `Text { content, style: Body|Title|Caption|Muted }`
- `Icon { name: IconId, size }` — references built-in icon atlas
- `Button { label_or_icon, on_click: ActionId }`
- `Input { value, placeholder, on_change }`
- `Checkbox { value, on_toggle }`
- `Spacer { flex: u8 }` — expands to fill
- `Divider`

**Compound (built from leaves, lives in SDK):**
- `Toolbar`, `StatusBar`, `Sidebar`, `List`, `Breadcrumb`, `IconGrid`

**Escape hatch — `Canvas`:**
- `Canvas { width, height, pixels: BGRA }` — only as a leaf inside a widget tree, never the whole window
- App writes to its WASM heap; `npk_canvas_commit(ptr, w, h, canvas_cap)` copies into a layer texture (~3ms for 4K via `rep movsq`)
- **Hard caps:** max 4096×4096 px, max 64 MB pixels total per app
- No drawing API — App ships finished pixels or nothing
- Requires a separate `CANVAS` capability (see below) — least privilege: file browser doesn't get one

**Modifiers** (chained on any widget):
- `.padding(n)`, `.margin(n)`
- `.background(Token)`, `.border(Token, width, radius)`
- `.opacity(0.0..1.0)`
- `.transition(Spring | Linear { ms })` — declares this widget should animate when its props change
- `.on_click(ActionId)`, `.on_hover(ActionId)`

---

## Typography

Two font systems, used in different contexts.

**System UI font — Inter (OFL):**
- Loaded at boot from `sys/fonts/inter-regular.ttf` and `sys/fonts/inter-bold.ttf` in npkFS
- BLAKE3-verified before use; updateable via OTA without kernel rebuild
- Rasterized via `fontdue` crate (no_std, ~2k LOC, MIT/Apache)
- Grayscale anti-aliasing (subpixel-AA not used — fine on HiDPI, irrelevant on 4K)
- Glyph atlas in GGTT slab; LRU eviction per font/size combo
- Fixed style tokens: `Title` (24px), `Body` (14px), `Caption` (11px), `Muted` (14px, dimmed). All scaled by HiDPI factor.

**Terminal mono font — Spleen (BSD-2):**
- Existing system: 8×16, 16×32, 32×64 bitmap (`gui/font.rs`)
- Used by `loop`, `top`, REPLs — anywhere a terminal cell grid is correct
- Apps that want the terminal aesthetic use `Text::mono(..)` modifier

**Decision rationale:** Inter for chrome/UI gives modern look; Spleen for terminals stays correct and pixel-perfect.

---

## Icons

**Source:** Phosphor (MIT) — curated subset compiled into atlas at build time.

**Pipeline:**
```
icons/phosphor/*.svg  ─┐
                       │  build.rs (rasterize via resvg, host-side only)
                       ▼
kernel/src/gui/icons.rs (generated)
   ├─ pub enum IconId { Folder, File, ArrowLeft, ... }
   └─ static ATLAS: &[u8] = &[ ... ];   // alpha-only, packed
```

**Sizes:** 16, 24, 32, 48, 64 px logical (so 32/48/64/96/128 actual at 2× HiDPI).

**Format:** alpha channel only (1 byte/pixel). Color comes from theme token at composite time. ~64 KB for 60 curated icons × 5 sizes.

**Adding an icon:** add SVG to `icons/phosphor/`, add variant to `IconId`, rebuild. No runtime SVG parsing — that path was rejected (subset parser is fragile, full parser is huge).

---

## Theme tokens

App **never** specifies hex colors. It uses tokens; compositor resolves against active palette (extracted from wallpaper).

```rust
#[repr(u8)]
enum Token {
    // Surfaces
    Surface          = 0,    // window background
    SurfaceElevated  = 1,    // cards, dialogs
    SurfaceMuted     = 2,    // sidebar, secondary regions

    // Text
    OnSurface        = 3,    // primary text on Surface
    OnSurfaceMuted   = 4,    // secondary text
    OnAccent         = 5,    // text on Accent button

    // Accent
    Accent           = 6,    // primary action color
    AccentMuted      = 7,    // hover/inactive variants

    // Semantic
    Border           = 8,
    Success          = 9,
    Warning          = 10,
    Danger           = 11,
}
```

Tokens map to indices into the existing 16-color `PALETTE` (`gui/color.rs`). Theme change = repaint all layer textures with new palette, no app involvement.

**ABI rule:** token integer values are stable forever. New tokens may be appended; existing values must never be reassigned. Apps compiled against v1 must keep working under v2.

---

## Layout: flexbox-lite

Strict subset, no overflow surprises:
- Row/Column with `spacing`, `align: Start|Center|End|Stretch`
- `Spacer { flex: u8 }` for proportional fill
- `padding`, `margin` per node
- `min_size`, `max_size` per node
- No floats, no absolute positioning, no z-index (use `Stack`)
- No percent units initially (only px and `flex`)

Implementation: ~500 LOC, single-pass for trivial trees, two-pass when `Spacer`/`Stretch` present.

---

## Diff + redraw strategy

Each widget node has:
- **Structural ID** = path from root (`Column[0].List.row[12]`)
- **Content hash** = blake3 over serialized props (4 bytes is enough)

Diff:
- ID exists in old + new with same hash → reuse cached layer texture, skip rasterize
- ID exists in both with different hash → re-rasterize this node only
- ID only in new → allocate texture, rasterize
- ID only in old → free texture

Compositor maintains `HashMap<NodeId, LayerTexture>` **per app** — survives across commits, freed on app exit.

**Why per-app, not global:** sharing identical icon textures across apps would save ~100 KB but introduce side-channel risk (cache-timing reveals what other apps are rendering) and complicate eviction. Premature optimization rejected.

**Result:** typing in an `Input` re-rasterizes one node, not the whole window. Scrolling a `List` blits cached row textures with a Y-offset, no rasterize.

---

## Animation

Pure compositor concern. App declares intent only.

```rust
Button::new("Open")
    .background(if self.hovered { Token::AccentMuted } else { Token::Accent })
    .transition(Spring::default())
```

When prop changes between two commits, compositor interpolates over N frames (spring physics or linear `ms`). App does **not** call render every frame; compositor self-schedules redraw while interpolating.

**Determinism:** all interpolation uses **integer/tick-based math**, not floats. Same approach as the existing swap-animation (`shade/compositor.rs`). Reasoning: float behavior across cores + variable wakeup latency leads to visible non-determinism. Spring physics implemented as fixed-point (Q16.16).

Animation tick = compositor runs at 60Hz **only while interpolations are active**. Otherwise dirty-driven (event → render → blit → idle).

---

## Capabilities

Two distinct rights, delegated separately at app spawn:

- **`RENDER`** — required to call `npk_scene_commit`. Granted to every windowed app.
- **`CANVAS`** — required to call `npk_canvas_commit`. Granted only to apps that declare a Canvas need (image viewer, chart, video player). File browser, settings, text editor → no `CANVAS` cap. Least privilege.

Both follow the existing capability model: 256-bit token, temporal scope, transitive revocation, audit logged.

---

## Host functions (new)

```c
// Commit a widget tree. `bytes` is postcard-serialized, version-prefixed.
// Returns 0 on success, -1 on parse error / version mismatch / cap denied.
i32 npk_scene_commit(const u8* bytes, u32 len, u32 render_cap);

// Commit pixel data for a Canvas leaf. width/height in pixels, BGRA32.
// canvas_id matches the Canvas{ id } in the last scene_commit.
// Returns 0 on success, -1 on size cap exceeded / cap denied.
i32 npk_canvas_commit(u32 canvas_id, const u8* pixels, u32 width, u32 height, u32 canvas_cap);

// Read input event for this app (key, mouse, focus).
i32 npk_event_poll(u8* buf, u32 buf_max);
i32 npk_event_wait(u8* buf, u32 buf_max, u32 timeout_ms);

// Optional: query current theme tokens (so app can pick icon variant).
u32 npk_theme_token(u32 token_id);
```

`npk_print` / `npk_clear` stay for terminal-style apps (`top`, REPLs).

---

## ABI stability & future-proofing

**Lock these in before writing the first line of widget code.** Mistakes here are expensive later.

### Wire format versioning

First byte of every `npk_scene_commit` payload is `WIRE_VERSION: u8`. Compositor rejects unknown versions with `-1`. App SDK checks return value, can fall back or report.

```
[ version: u8 ][ postcard-serialized Widget tree ]
```

v1 = `0x01`. Future versions either bump the byte (breaking change) or add optional fields at end of structures (postcard handles forward compat for `Option<T>` at struct tail).

### Token enum stability

`Token` integer values frozen on v1 release. New tokens appended only. Removing or renumbering = ABI break = wire-version bump.

### IconId stability

Same rule: `IconId` is `#[repr(u16)]`, values frozen. Adding new icons appends.

### GGTT partition map (lock before slab implementation)

Decide and document numerical addresses **before** writing the allocator. Moving them later breaks every cached pointer.

```
0x0000_0000 - 0x0100_0000   GGTT scratch (unused, reserved)
0x0100_0000 - 0x0400_0000   Framebuffer (existing, 48 MB)
0x0400_0000 - 0x0500_0000   BCS infrastructure (existing: ring, LRC, HWSP, test)
0x0500_0000 - 0x0600_0000   Glyph atlases (16 MB — Inter + variants)
0x0600_0000 - 0x0700_0000   Icon atlas (16 MB — fixed, set at boot)
0x0700_0000 - 0x4000_0000   Layer texture slab (~916 MB)
```

Slab buckets: 1 KB, 4 KB, 16 KB, 64 KB, 256 KB, 1 MB, 4 MB. Free-lists per bucket. LRU eviction across all buckets when slab > 80 % full.

### Capability ABI

`RENDER` and `CANVAS` are separate token kinds. New capabilities may be added (e.g. `MIC`, `CAMERA` later); existing ones never repurposed.

---

## What apps explicitly do NOT get

- No `npk_draw_*` immediate-mode functions
- No font loading from app side (system fonts only, fixed style tokens)
- No custom shaders
- No GPU texture handles
- No window-decoration control (compositor owns chrome)
- No raw framebuffer access
- No animation scripting (declarative only)
- No Canvas without explicit `CANVAS` capability
- No way to position Canvas as a "transparent overlay window" — it's a leaf, period

Reduces attack surface, prevents per-app drift, makes 4K-scaling and theme-changes universal.

---

## Implementation phases

Order matters — each phase produces something runnable.

### P10.0 — ABI freeze (1 day, paper-only)
- Document GGTT partition map (above) — committed to source as `gpu/ggtt_layout.rs` constants
- Freeze `Token` enum values + `IconId` enum scaffolding (empty variants ok)
- Define wire-version byte = `0x01`
- Write down capability split (`RENDER` / `CANVAS`)
- **Deliverable:** `kernel/src/shade/widgets/abi.rs` with the constants, no logic

### P10.1 — SDK + serialization (1 week, no kernel changes yet)
- `tools/wasm/sdk/widgets/` — new shared crate, no_std + alloc
- Define `Widget` enum, `Modifier`, `Event`, `Action`, `Token`, `IconId`
- Postcard serialization with version-byte prefix
- Unit tests round-tripping trees, including version-mismatch rejection
- **Deliverable:** SDK compiles, tree can be serialized to bytes

### P10.2 — Compositor receiver + dummy renderer (3–5 days)
- `kernel/src/shade/widgets/mod.rs` — new module
- `npk_scene_commit` host fn — version-check, deserialize, log to serial, no render yet
- `tools/wasm/files-stub/` — dummy app, sends one tree on launch
- **Deliverable:** see deserialized tree printed on serial when app runs

### P10.3 — Layout engine (1 week)
- `kernel/src/shade/widgets/layout.rs` — flexbox-lite
- Assigns absolute x/y/w/h to every node
- Tested standalone with snapshot tests against known trees
- **Deliverable:** layout pass produces correct geometry, dumped to serial

### P10.4 — GGTT slab allocator (4–5 days)
- `kernel/src/gpu/ggtt_slab.rs` — fixed-bucket slab, LRU eviction
- Uses partition from P10.0
- Unit-tested for allocation/free patterns + fragmentation behavior
- **Deliverable:** slab can serve thousands of alloc/free cycles without leak or fragmentation

### P10.5 — Layer texture rasterization (1 week)
- `kernel/src/shade/widgets/render.rs` — walks laid-out tree
- Reuses `gui/render.rs` primitives, target = per-node layer buffer in slab
- BCS blits layer textures into window content region
- **Deliverable:** static file-browser tree (no font yet, placeholder rects for text) renders in a window

### P10.6 — fontdue + Inter integration (4–5 days)
- Add `fontdue` to kernel `Cargo.toml`
- `kernel/src/gui/text.rs` — glyph rasterization, atlas in GGTT
- Inter Regular + Bold loaded from npkFS at boot, BLAKE3-verified
- `Text` widget renders real text
- **Deliverable:** file-browser tree shows actual Inter text at correct size

### P10.7 — Diff + cache (4–5 days)
- Node ID + content hash
- Per-app `HashMap<NodeId, LayerTexture>` survives across commits
- Skip rasterize when hash unchanged
- **Deliverable:** typing in Input re-rasterizes only that node (verifiable via debug overlay)

### P10.8 — Event routing (3 days)
- Mouse: hit-test laid-out tree, find topmost `on_click` widget
- Keyboard: focus stack, Tab navigation
- `npk_event_poll` / `npk_event_wait` host fns
- **Deliverable:** clicking a button fires the action in the app

### P10.9 — Animation (1 week)
- Spring physics + linear timing in compositor, fixed-point Q16.16
- Self-scheduling 60Hz tick while active
- Interpolate `background`, `opacity`, `padding`, position deltas
- **Deliverable:** hover state on buttons fades smoothly, deterministic

### P10.10 — Icon atlas (3 days)
- `build.rs` rasterizes curated Phosphor SVGs at compile time
- 5 size variants (16/24/32/48/64), alpha-only, packed
- `IconId` enum populated, atlas embedded as `static`
- Atlas uploaded to GGTT at boot
- **Deliverable:** file-browser shows real icons in correct theme color

### P10.11 — Canvas (4–5 days)
- `npk_canvas_commit` host fn
- Size caps enforced (4K × 4K, 64 MB total per app)
- `CANVAS` capability check
- BGRA copy from WASM heap → layer texture in slab
- **Deliverable:** image-viewer stub displays a PNG decoded in the app

### P10.12 — File browser app (1 week)
- `tools/wasm/files/` — real app, walks npkFS, opens via intent
- **Deliverable:** working file browser, GNOME-feel, no `CANVAS` cap needed

**Total: ~6–7 weeks for a polished v1 incl. file browser + Canvas.**

---

## What this is **not**

- Not a web engine. No CSS, no DOM, no JS.
- Not a vector renderer. Lines/curves only via icons or Canvas.
- Not a windowing API. Shade owns windows; widgets are the window's contents.
- Not Wayland. No protocol, no surfaces, no roles. One host call per frame, period.
- Not Skia. Not Cairo. Smaller, narrower, kernel-internal.

---

## Long-term: what this enables

- File manager, settings app, text editor, image viewer, calendar — all share the same widget set + theme
- Apps automatically inherit wallpaper-driven theme changes
- HiDPI is free — compositor scales layout, no per-app code
- Animations are free — compositor interpolates, no per-app code
- Accessibility hooks (focus traversal, screen-reader tree) become possible later without app changes
- Phase 11 AI: LLM emits widget trees directly — perfect declarative target

---

## Decisions log (resolved)

These were the open questions; closed with reasoning so we don't re-litigate later.

| Question | Decision | Why |
|---|---|---|
| Font backend | `fontdue` + Inter (OFL) for UI; Spleen kept for terminals | Modern UI typography needs vector fonts; Spleen looks right in mono/terminal contexts. Inter is OFL, no Google branding. |
| Font storage | npkFS, BLAKE3-verified | Matches content-addressing principle; OTA-updatable without kernel rebuild |
| Icon source | Phosphor (MIT), pre-rasterized at build time, alpha-only | Runtime SVG parsing is fragile or huge; alpha + theme token gives free re-coloring |
| Icon sizes | 16/24/32/48/64 px logical (5 variants) | Covers 1× and 2× HiDPI cleanly |
| Serializer | Postcard with version byte | Smallest, no_std, Rust-native; version byte enables forward evolution |
| GGTT allocator | Slab with fixed buckets (1KB–4MB), LRU eviction | Bump allocator fragments under churn; slab is bounded |
| Canvas in v1 | Yes, but tightly scoped: leaf-only, size-capped, separate capability | Need it for image viewer demo; constraints prevent GDI-style drift |
| Cache scope | Per-app, no cross-app sharing | Side-channel concerns + eviction complexity outweigh ~100KB savings |
| Capability split | `RENDER` and `CANVAS` separate | Least privilege: file-browser doesn't need to draw pixels |
| Animation math | Fixed-point Q16.16, tick-based | Float non-determinism across cores breaks visual consistency |

---

*Last updated after P10.0 design pass. Implementation starts at P10.0 (ABI freeze).*
