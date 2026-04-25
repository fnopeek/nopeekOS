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

## Architecture & Responsibilities

Who owns what. The layering is strict — crossing it means a wire-version bump or a capability addition.

```
┌──────────────────────────────────────────────────────────────────┐
│  WASM App (tools/wasm/<name>/)                                   │
│    owns:    state, render() → Widget tree, Action handlers       │
│    NEVER:   pixels, fonts, layout math, animation, theme colors, │
│             input routing, focus, window chrome                  │
├──────────────────────────────────────────────────────────────────┤
│  SDK crate  nopeek_widgets  (in-app Rust, no_std + alloc)        │
│    owns:    Widget/Modifier/Event/Action types,                  │
│             postcard serialize + version byte prefix,            │
│             compound widgets (List, Toolbar, Sidebar, …),        │
│             App trait + event loop helper                        │
│    NEVER:   kernel side, host fn internals                       │
╞══════════════════════════════════════════════════════════════════╡  ← WASM sandbox boundary
│  Host functions  (capability-gated, kernel/src/wasm.rs)          │
│    npk_scene_commit    RENDER cap      → tree bytes into kernel  │
│    npk_canvas_commit   CANVAS cap      → BGRA pixels into slab   │
│    npk_event_poll/wait RENDER cap      → input events out        │
│    npk_theme_token     RENDER cap      → current palette query   │
├──────────────────────────────────────────────────────────────────┤
│  Compositor  (kernel/src/shade/widgets/)                         │
│    version-check → deserialize → layout (flexbox-lite) → classify │
│    comp boundaries → diff → compute dirty tile + layer sets →    │
│    schedule raster tasks → collect targets → submit blit list.   │
│    Owns: animation interp, focus, hit-test, theme resolution,    │
│    per-app tile+layer cache, Canvas size-cap enforcement.        │
├──────────────────────────────────────────────────────────────────┤
│  Rasterizer  trait  (kernel/src/shade/widgets/raster/)           │
│    v1:  CpuRasterizer   — fontdue + gui/render.rs primitives     │
│    v2:  XeRenderRasterizer — Gen 12 RCS + fragment shaders       │
│    Draws into RasterTarget (tile or comp layer, same interface). │
│    Swappable at boot; compositor holds Box<dyn Rasterizer>.      │
├──────────────────────────────────────────────────────────────────┤
│  GGTT slab  (kernel/src/gpu/ggtt_slab.rs)                        │
│    owns:    tile + comp-layer allocation, LRU eviction           │
│             (off-screen tiles evicted first)                     │
│    fallback: system-RAM-backed GGTT pages when slab > 80% full   │
├──────────────────────────────────────────────────────────────────┤
│  BCS compositor  (existing, kernel/src/gpu/intel_xe.rs)          │
│    owns:    dirty tiles + comp layers → framebuffer via          │
│             XY_FAST_COPY_BLT, batched submission, vblank flip    │
└──────────────────────────────────────────────────────────────────┘
```

**Who does what, per pipeline stage:**

| Stage                          | App | SDK | Compositor | Rasterizer | GGTT | BCS |
|--------------------------------|:---:|:---:|:----------:|:----------:|:----:|:---:|
| Build tree                     |  ✓  |  ✓  |            |            |      |     |
| Serialize + ver byte           |     |  ✓  |            |            |      |     |
| Capability check               |     |     |     ✓      |            |      |     |
| Deserialize                    |     |     |     ✓      |            |      |     |
| Layout (x/y/w/h)               |     |     |     ✓      |            |      |     |
| Classify comp boundaries       |     |     |     ✓      |            |      |     |
| Diff (ID + hash)               |     |     |     ✓      |            |      |     |
| Compute dirty tile + layer set |     |     |     ✓      |            |      |     |
| Allocate tile / comp layer     |     |     |     ✓      |            |  ✓   |     |
| Rasterize into target          |     |     |            |     ✓      |      |     |
| Cache tile / layer             |     |     |     ✓      |            |  ✓   |     |
| Animation interpolate          |     |     |     ✓      |            |      |     |
| Theme → concrete color         |     |     |     ✓      |            |      |     |
| Hit-test mouse / focus         |     |     |     ✓      |            |      |     |
| Submit blit list               |     |     |            |            |      |  ✓  |
| Evict tile / layer (LRU)       |     |     |     ✓      |            |  ✓   |     |

**Key property:** every stage has a single owner. The App owns zero pixels; the Compositor owns zero rasterization; the Rasterizer is stateless per call; the GGTT slab owns zero layout. Adding a GPU backend means swapping **one** row (Rasterize into target) — nothing else changes.

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
   ├─ Classify nodes into composition boundaries (opacity<1, transition,
   │     blur/shadow/effect, Canvas, Popover/Tooltip/Menu) → own layer.
   │     All other nodes rasterize into the window's tile grid.
   ├─ Diff against previous tree (structural ID + content hash)
   │     → dirty set of tiles (from node rects) + dirty composition layers
   ├─ Schedule raster tasks onto worker cores (one tile = one task)
   │     └─ Worker walks tree, rasterizes contained nodes into tile buffer
   │        (CPU; fontdue + gui/render.rs) via Rasterizer trait
   │
   ▼
GPU (BCS XY_FAST_COPY_BLT)
   └─ Blit dirty tiles + composition layers → framebuffer (few large blits)
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

**Reserved for later (variant slot held in v1 enum, implementation TBD):**
- `Popover { anchor, child }` — content that may escape window bounds (dropdowns, date pickers, color pickers)
- `Tooltip { text, anchor }` — hover-triggered text overlay
- `Menu { items }` — context menu, keyboard-navigable, escapes window

These are declared in the v1 `Widget` enum with `#[allow(dead_code)]` placeholders. The compositor rejects them with a log message until implemented. **Reason:** overlay/out-of-bounds rendering cannot be retrofitted without breaking every serialized tree from v1. Holding the variant slot now costs nothing and avoids a wire-version bump later.

**Modifiers** (chained on any widget):
- `.padding(n)`, `.margin(n)`
- `.background(Token)`, `.border(Token, width, radius)`
- `.opacity(0.0..1.0)`
- `.transition(Spring | Linear { ms })` — declares this widget should animate when its props change
- `.on_click(ActionId)`, `.on_hover(ActionId)`
- `.role(Role)` — accessibility override (see A11y section; held slot, compositor reads but v1 does not consume)

**Reserved modifier slots (v2+, held in v1 enum):**
- `.blur(radius: u8)` — Gaussian blur behind widget (acrylic/glass effect)
- `.shadow(offset: Point, blur: u8, token: Token)` — drop shadow
- `.effect(EffectId)` — named GPU effect, extensibility escape

CPU rasterizer treats these as no-ops in v1. GPU rasterizer (Phase 12+) implements them. No wire-version bump required when they light up.

---

## Typography

Two font systems, used in different contexts. The UI font is **implemented in P10.1** — not deferred to a late phase. Layout, rasterization, and cache pipelines use real font metrics from day one; there are no placeholder-rect text stages.

**System UI font — Inter Variable (OFL):**
- One file, `sys/fonts/inter-variable.ttf` (~800 KB) — contains the full weight axis (100–900) plus slant. Replaces separate Regular/Bold TTFs.
- Loaded at boot, BLAKE3-verified, OTA-updateable without kernel rebuild.
- Rasterized via `fontdue` crate (no_std, ~2k LOC, MIT/Apache). fontdue supports variable-axis natively — zero extra code to use weights.
- Grayscale anti-aliasing (subpixel-AA not used — fine on HiDPI, irrelevant on 4K).
- **Hinting enabled** — sharp small-size text on lower-DPI displays (e.g. VirtualBox dev window).
- Glyph atlas in GGTT slab, keyed by `(glyph_id, size, weight)` tuple; LRU eviction per combo.

**Style tokens — mapped to variable-font weights and real metrics:**

| Token      | Size | Weight | Use                                 |
|------------|-----:|-------:|-------------------------------------|
| `Title`    | 24px |    600 | Window titles, dialog headers       |
| `Body`     | 14px |    400 | Default UI text                     |
| `Muted`    | 14px |    400 | Body + 60% alpha (secondary text)   |
| `Caption`  | 11px |    500 | Labels, captions — slightly heavier to stay legible at small size |
| `Mono`     |    — |      — | Routes to Spleen (terminal aesthetic) |

All sizes multiplied by HiDPI scale factor at raster time.

**Layout uses real font metrics, not hardcoded values:**
- `ascent`, `descent`, `line_gap` read from Inter's `hhea` table → correct vertical rhythm per weight
- `advance_width` per glyph from `hmtx` → text measurement without rasterizing
- `cap_height`, `x_height` from `OS/2` table → baseline-aligned layout between `Title` and `Body`

**OpenType features enabled by default:**
- `tnum` (tabular numerals) — digits have equal advance width. Critical for lists, tables, clock, counters — numbers don't jitter.
- `kern` (kerning) — Inter's pair-adjustment table respected.

Future (v2, no ABI change required): `liga` (standard ligatures), `ss01–ss08` (stylistic sets).

**Terminal mono font — Spleen (BSD-2):**
- Existing system: 8×16, 16×32, 32×64 bitmap (`gui/font.rs`)
- Used by `loop`, `top`, REPLs — anywhere a terminal cell grid is correct
- Apps that want the terminal aesthetic use `Text::mono(..)` modifier

**Decision rationale:** Inter Variable gives a 2025 premium look — one file covers all weights smoothly, tabular numerals remove UI jitter, real metrics give correct baseline rhythm. Zero extra dependencies beyond fontdue. Spleen stays for terminals where bitmap mono is correct.

**What's explicitly deferred to v2 (no ABI impact, same `Text { content: String }` API):**
- OpenType shaping via `rustybuzz` — ligatures (`fi`, `→`), Arabic/Devanagari/Thai, BiDi. ~2 weeks of work, not required for "modern Latin UI".
- Color emoji (COLRv1) — needs RGBA atlas, ~1 week. Monochrome fallback via Inter's Unicode coverage suffices for v1.
- Font fallback chain (CJK, symbols) — reserved via `FontId` in `TextStyle`.

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

## Raster granularity — tiles + composition layers

**Critical design decision.** The naive approach (one texture per widget node) dies under real workloads: thousands of 1-KB GGTT allocations, slab fragmentation, and hundreds of tiny BCS blits per frame. Browser engines (Blink, WebKit) learned this around 2013 and moved to tiled rasterization. We do the same from v1.

**Two texture kinds, two purposes:**

### 1. Tiles — the workhorse

A window's content is rasterized into a **fixed grid of tiles** in the GGTT slab. Tiles are geometry-driven, not widget-driven — they're stable even when the widget tree is rebuilt.

- **Tile size: 512×512 actual pixels** (256×256 logical at 2× HiDPI), BGRA32 → **1 MB per tile**, primary slab bucket
- 4K window (3840×2160): 8×5 = 40 tiles × 1 MB = 40 MB per full window
- Off-screen tiles (e.g. scroll content below fold) LRU-evicted individually — not the whole window

A worker core walks the laid-out tree, collects all leaves whose rect intersects a given tile, and rasterizes them **directly into the tile's pixel buffer**. No per-leaf texture. No per-leaf allocation.

### 2. Composition layers — the exceptions

A node gets its **own** texture (not rasterized into tiles) only at a **composition boundary**:

- `.transition(..)` in-flight — interpolate without retouching tiles every frame
- `.opacity(x)` with `x < 1.0` — alpha-blend cleanly with underlying tiles
- `.blur` / `.shadow` / `.effect` (v2) — GPU pass operates on own surface
- `Canvas` widget — app-supplied pixels, already its own surface
- `Popover` / `Tooltip` / `Menu` — must render outside window bounds

Composition layers use the slab with appropriate bucket sizes (small for hover-buttons, large for Canvas up to the per-app 64 MB cap). Typical window: **40 tiles + 0–3 composition layers**. Not 500 leaf textures.

### Diff is still at node granularity — but drives tile dirtying

Each widget node has:
- **Structural ID** = path from root (`Column[0].List.row[12]`)
- **Content hash** = blake3 over serialized props (4 bytes is enough)

Pipeline:

| Diff result                         | Consequence                                                          |
|-------------------------------------|----------------------------------------------------------------------|
| ID exists old+new, same hash        | Node unchanged; skip                                                 |
| ID exists both, hash changed        | Mark tiles intersecting node's rect as dirty                         |
| ID only in new                      | Mark tiles intersecting rect as dirty (and allocate comp layer if boundary) |
| ID only in old                      | Mark tiles intersecting old rect as dirty (free comp layer if was one) |

**Dirty set is a set of `TileId`, plus a set of `LayerId`.** Worker cores pick up dirty entries in parallel. A tile re-raster walks the tree once and writes all contained nodes into the tile buffer.

### Cache state (per app)

```rust
struct AppCache {
    tiles:  HashMap<TileId, TileTexture>,   // keyed by (window_id, tile_x, tile_y)
    layers: HashMap<NodeId, LayerTexture>,  // only composition-boundary nodes
}
```

Per-app, not global. Sharing identical textures across apps would save ~100 KB but introduce side-channel risk (cache-timing reveals what other apps render) and complicate eviction. Rejected.

### What this means in practice

- **Typing in an `Input`:** Input's rect intersects 1 tile → re-raster that tile (~500 μs on modern CPU) → 1 blit. No other work.
- **Scrolling a `List`:** tiles on-screen stay cached; newly-revealed tiles rasterize on demand (worker cores in parallel). Scrolling is effectively free once tiles are warm.
- **Hovering a `Button` with `.transition`:** button is a composition layer (boundary detected on first render with `.transition`). Tiles underneath unchanged. Only the button's layer re-raster + alpha blend during interpolation.
- **Theme change:** all tiles + layers invalidated at once (palette lookup happens at raster time, not ABI time) → worker cores re-raster in parallel; typically <50 ms for a full desktop.

**BCS cost per frame:** ~40 tile blits + a few composition-layer blits per window, batched into a single ring submission. No tile-storm.

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

## Rasterizer abstraction

The compositor never calls into `gui/render.rs` or `fontdue` directly. Every raster operation routes through a trait. The target is a **`RasterTarget`** — either a tile or a composition layer, both look the same to the rasterizer (a pixel buffer with an origin offset).

```rust
pub struct RasterTarget<'a> {
    pub pixels: &'a mut [u32],      // BGRA32
    pub stride: u32,                // pixels per row
    pub size:   Size,               // width, height in pixels
    pub origin: Point,              // this target's top-left in window coords
                                    //   — rasterizer subtracts to get local coords
                                    //   — tiles have origin = (tile_x*512, tile_y*512)
                                    //   — comp layers have origin = node's layout rect top-left
    pub scale:  u8,                 // HiDPI factor (1 or 2)
    pub palette: &'a Palette,       // Token → concrete BGRA
}

pub trait Rasterizer: Send + Sync {
    fn clear(&mut self, t: &mut RasterTarget, color: Token);
    fn rect(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill);
    fn text(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, p: Point);
    fn icon(&mut self, t: &mut RasterTarget, id: IconId, size: u16, color: Token, p: Point);
    fn canvas_copy(&mut self, t: &mut RasterTarget, src: &[u8], w: u16, h: u16);

    // v2+: default no-op on CPU backend, implemented by GPU backend
    fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}
    fn shadow(&mut self, _t: &mut RasterTarget, _r: Rect, _s: Shadow) {}
    fn effect(&mut self, _t: &mut RasterTarget, _r: Rect, _id: EffectId) {}
}
```

`Rect` and `Point` in the trait are in **window coordinates**. The rasterizer subtracts `origin` to get the target-local position, and clips draws to `size`. Drawing a node that overlaps a tile boundary Just Works — the left tile clips the right half away, the right tile clips the left half. No per-node coordinate gymnastics in the compositor.

- **v1 — `CpuRasterizer`** — wraps `gui/render.rs` primitives + fontdue glyph cache. Clipping + origin-subtract in software.
- **v2+ — `XeRenderRasterizer`** (Phase 12) — Intel Xe Gen 12.2 RCS (render engine, not BCS), fragment shaders for blur/shadow/gradients, GPU-side text via SDF atlas. Scissor rect set from target `size`, origin applied via transform matrix.

Compositor holds `Box<dyn Rasterizer>`, selected at boot. Future: per-window backend selection (experimental GPU path with CPU fallback).

**Non-negotiable:** no call site in the widget pipeline references CPU or GPU specifics. Switching rasterizer requires zero changes to layout, diff, event, or cache code. This is the single most important abstraction for future HW acceleration.

---

## Threading

Widget pipeline maps onto the Phase 9 event-driven SMP model. The tile-based design is naturally parallel — **one tile = one independent raster task**.

| Stage                                | Runs on            | Why                                            |
|--------------------------------------|--------------------|------------------------------------------------|
| `scene_commit` host call             | worker (WASM app)  | app already runs on its worker core            |
| Deserialize + layout + diff          | worker (same)      | CPU-bound, bounded by tree size                |
| Compute dirty tile set + layer set   | worker (same)      | O(nodes_changed); cheap                        |
| Rasterize each dirty tile            | any worker (spawn) | embarrassingly parallel — tiles are independent|
| Rasterize each dirty comp layer      | any worker (spawn) | same                                           |
| Slab alloc/free                      | any core           | slab is Mutex-protected, contention rare       |
| BCS blit list submit + doorbell      | Core 0             | Phase 9 constraint, Core 0 owns ring           |
| Vblank wait + PLANE_SURF flip        | Core 0             | same                                           |

**Parallelism:** 40 dirty tiles spread across 3 worker cores → each core does ~13 tiles × ~500 μs = 6.5 ms wall clock. Well within a 16.6 ms frame budget at 60 Hz. In the typical case (one tile dirty per keystroke), it's a single ~500 μs task.

**Constraints:**
- Core 0 never rasterizes. Never.
- Workers complete → produce `BlitRequest { src: TileTexture|LayerTexture, dst: Rect }` list → Core 0 consumes via existing event queue.
- Commit coalescing: if an app commits faster than raster completes, only the latest tree matters. Drop intermediates (same pattern as mouse-move events in Phase 9). Dirty sets from dropped commits merge into the live one — no visual inconsistency.

**Backpressure:** `npk_scene_commit` returns 0 immediately after queuing. Render happens asynchronously. App does not wait for pixels — next `event_wait` may return before the previous commit has flipped. This is intentional: apps stay responsive, compositor self-paces.

---

## Accessibility (A11y) — role reservation

Every widget node carries a `role: Role` tag from v1. v1 does not consume it — but freezing the enum now means screen readers, keyboard traversal, and UI automation can be added later without a wire-version bump.

```rust
#[repr(u8)]
#[non_exhaustive]
pub enum Role {
    None        = 0,   // decorative only, skip in traversal
    Button      = 1,
    Link        = 2,
    TextInput   = 3,
    List        = 4,
    ListItem    = 5,
    Heading     = 6,
    Image       = 7,
    Separator   = 8,
    Group       = 9,
    Status      = 10,
    // appended only — values frozen forever
}
```

**Defaults** (inferred from widget kind):
- `Button` → `Role::Button`
- `Input` → `Role::TextInput`
- `Text { style: Title }` → `Role::Heading`
- `List` → `Role::List`, inner rows → `Role::ListItem`
- `Divider` → `Role::Separator`
- `Icon` standalone → `Role::Image`; inside `Button` → subsumed

App may override via `.role(Role::X)` modifier — e.g. a custom clickable `Row` that behaves as a button.

**Why now:** retrofit a11y later means either every app rebuilt or heuristic DOM-like guessing. AppKit learned this the hard way — ~15 years of VoiceOver edge cases caused by missing role hints in legacy controls. Cost to reserve the enum now: one `Role` byte per node in the wire format (~200 bytes per typical tree). Cost to retrofit: catastrophic.

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

### Enum variant ordering (postcard wire position)

Postcard serializes enum variants by **position**, not by name. Inserting a variant in the middle of `Widget`, `Modifier`, `Event`, `Action`, or any other ABI-visible enum breaks every tree that was serialized before the change.

Rules (enforced by code review + a `check_abi.rs` test):
- All ABI-visible enums carry `#[non_exhaustive]`
- New variants **appended only** — never inserted, never reordered
- Removing a variant = wire-version bump
- Reserved variants use `#[allow(dead_code)]` placeholders to hold the slot (see `Popover`, `Tooltip`, `Menu`, `.blur`, `.shadow`, `.effect`)
- `#[repr(u8)]` or `#[repr(u16)]` where the variant index is part of a public ABI (`Token`, `IconId`, `Role`)

Covered enums: `Widget`, `Modifier`, `Event`, `Action`, `Token`, `IconId`, `Role`, `Fill`, `TextStyle`, `EffectId`, `KeyCode` (already stable from Phase 8).

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
0x0500_0000 - 0x0600_0000   Glyph atlases (16 MB — Inter Variable weight/size cells)
0x0600_0000 - 0x0700_0000   Icon atlas (16 MB — fixed, set at boot)
0x0700_0000 - 0x4000_0000   Tile + composition-layer slab (~916 MB)
```

**Slab bucket sizes and roles:**

| Bucket | Primary use                                          |
|-------:|------------------------------------------------------|
|   1 KB | (legacy reserved — not used in tile model)           |
|   4 KB | Small composition layers (e.g. hover-pill button)    |
|  16 KB | Mid composition layers (tooltip, small popover)      |
|  64 KB | Larger composition layers (dropdown menu)            |
| 256 KB | Small Canvas layers; large popover/menu              |
|   1 MB | **Primary — tiles (512×512 BGRA) and small Canvas**  |
|   4 MB | Large Canvas (up to 1024×1024 logical)               |

Typical residency: ~40 tiles per active window in the 1-MB bucket, ~0–3 composition layers per app in 4 KB–64 KB buckets. Free-lists per bucket. LRU eviction across all buckets when slab > 80 % full. Eviction prefers off-screen tiles over in-flight composition layers.

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

### Status snapshot

| Phase | Status | Version | Notes |
|---|---|---|---|
| P10.0 ABI freeze | ✅ done | v0.50.7 | All ABI enums frozen, check_abi.rs lock |
| P10.1 SDK + fonts | ✅ done | v0.51.0 | `nopeek_widgets` crate, Inter Variable via fontdue |
| P10.2 scene_commit | ✅ done | v0.54.0 | First end-to-end wire round-trip |
| P10.3 Layout | ✅ done | v0.55.0 | flexbox-lite with real Inter metrics |
| P10.4 GGTT slab | ✅ done | v0.56.0 | 912 MB region, 7 buckets, LRU |
| P10.5 Rasterizer | ✅ done | v0.57.0–.2 | CpuRasterizer, first visible pixels |
| P10.5b Widget windows | ✅ done | v0.58.0 | WindowKind split, per-window scenes, grid-native |
| — Widget polish | ✅ done | v0.58.1 | Rounded corners, Opacity modifier, theme integration |
| P10.6 Diff + cache | 🟡 partial | v0.59.0 | payload-hash skip (full tile diff needs tile subdivision) |
| P10.7 Events | 🟡 partial | v0.60.0 | Mouse hit-test + npk_event_poll; keyboard routing + blocking wait deferred |
| P10.8 Animation | 🟡 scaffold | v0.61.0 | Q16.16 math + tick(); no active consumers until tree-diff lands |
| P10.9 Icons | ✅ done | v0.62.0 | 18 Phosphor Regular, 149 KB atlas, OTA-updatable like font |
| drun launcher | ✅ done | v0.64.2 / drun 0.2.1 | Mod+D centred overlay, modal, keyboard nav — first real interactive widget app |
| P10.10 Canvas | ⏳ later | — | `npk_canvas_commit` + CANVAS cap, size-capped |
| P10.11 File browser | ⏳ later | — | Real Thunar-clone — the eventual capstone |

**Partial phases** mean "infrastructure shipped, full impl waits on a
prerequisite". None of them block the next phase's user-visible work.

### Open follow-ups (not-in-spec, tracked)

- ~~drun — first real interactive widget app~~ — ✅ shipped v0.64.2.
  Lists installed `sys/wasm/*` modules, keyboard nav, Enter launches.
  Uses `npk_window_set_overlay` + `npk_window_set_modal` to declare
  its window kind — kernel stays launcher-agnostic.
- ~~Keyboard routing to widget windows~~ — ✅ shipped v0.64.1.
  Focused widget-kind window receives `Event::Key`; the intent
  loop's `read_line_with_tab` bails out immediately via
  `focused_widget_id()`.
- **drun polish** — current rough edges: mouse-click selection
  missing, no search input, visual style minimal. Cosmetic pass
  planned next.
- **Kind conversion (Terminal ↔ Widget)** — `npk_spawn_module`
  currently always opens a terminal-kind window so `loop + <app>`
  semantics match. Widget-only apps launched via drun end up with
  an unused terminal chrome. `npk_window_set_overlay` should
  convert the window cleanly (update kind field, free terminal
  slot, install scene entry).
- **Keybind configurability** — only the launcher target is
  currently config-driven (`sys/config/launcher`). Full keybind
  mapping (all Mod+X combos → action/module) lives in a future
  `sys/config/keybinds`, replacing the hardcoded lookups in
  `shade::input::try_keybind_event`.
- **Tile subdivision + full diff cache** — second-pass P10.5 and
  P10.6 together. Needs 512×512 tile grid per window, per-tile
  content-hash, dirty-tile scheduler. Makes interactive apps cheap.
- **`npk_event_wait` blocking poll** — P10.7 follow-up, needs
  integration with idle/sleep path.
- **Window resize triggers scene re-layout** — `relayout_scene`
  exists, needs a `dirty` nudge from the resize handler so shade
  picks it up before next redraw.
- **Shade policy extraction to WASM** (post-Phase 10) — tiling
  algo, keybinds, theme palette, animation curves each become
  their own hot-reloadable module. ~750 LOC extractable. Explicit
  non-goal during Phase 10.

### P10.0 — ABI freeze (2 days, paper-only)  ✅
- Document GGTT partition map + slab bucket roles (above) — committed as `gpu/ggtt_layout.rs` constants
- Fix tile size (512×512 actual px, 1 MB per tile) as compile-time constant in `shade/widgets/tile.rs`
- Freeze `Token` enum values + `IconId` enum scaffolding (empty variants ok)
- Freeze `Role` enum values (all defaults documented per widget kind)
- Freeze `Widget` enum with reserved `Popover`/`Tooltip`/`Menu` slots (body types defined, impl path = `log + reject`)
- Freeze `Modifier` enum with reserved `.blur`/`.shadow`/`.effect`/`.role` slots
- Define `Rasterizer` trait signature + `RasterTarget` struct (header only, no impl)
- Define wire-version byte = `0x01`
- Write down capability split (`RENDER` / `CANVAS`)
- Enum-ordering rule captured in `check_abi.rs` compile-time test (variant-count assertions per enum)
- **Deliverable:** `kernel/src/shade/widgets/abi.rs` with the constants + trait signatures, no logic

### P10.1 — SDK + serialization + font metrics (1.5 weeks)  ✅
- `tools/wasm/sdk/widgets/` — new shared crate, no_std + alloc
- Define `Widget` enum, `Modifier`, `Event`, `Action`, `Token`, `IconId`, `Role`
- Postcard serialization with version-byte prefix
- Unit tests round-tripping trees, including version-mismatch rejection
- **Font system bootstrap (new):**
  - Add `fontdue` to kernel `Cargo.toml`
  - `kernel/src/gui/text.rs` — font loader, metrics API, weight-axis handling
  - Inter Variable loaded at boot from `sys/fonts/inter-variable.ttf`, BLAKE3-verified
  - Expose `advance_width(ch, style)`, `line_height(style)`, `ascent/descent/x_height/cap_height`
  - Glyph atlas module — structure only, no GGTT slab yet (comes in P10.4)
  - `tnum` + `kern` OpenType features enabled at load
- **Deliverable:** SDK compiles, trees serialize, font metrics queryable for layout

### P10.2 — Compositor receiver + dummy renderer (3–5 days)  ✅
- `kernel/src/shade/widgets/mod.rs` — new module
- `npk_scene_commit` host fn — version-check, deserialize, log to serial, no render yet
- `tools/wasm/files-stub/` — dummy app, sends one tree on launch
- **Deliverable:** see deserialized tree printed on serial when app runs

### P10.3 — Layout engine (1 week)  ✅
- `kernel/src/shade/widgets/layout.rs` — flexbox-lite
- Assigns absolute x/y/w/h to every node
- **Uses real font metrics from P10.1** — Text nodes measured via `advance_width`, not stubbed
- Baseline-aware row alignment (mix `Title` + `Body` in a row → correctly aligned)
- Tested standalone with snapshot tests against known trees
- **Deliverable:** layout pass produces correct geometry incl. real text measurements, dumped to serial

### P10.4 — GGTT slab allocator (4–5 days)  ✅
- `kernel/src/gpu/ggtt_slab.rs` — fixed-bucket slab, LRU eviction
- Uses partition + bucket sizes from P10.0
- **Primary bucket is 1 MB** (tiles). Smaller buckets for composition layers.
- Eviction priority: off-screen tiles first, composition layers last
- Glyph atlas migrated from heap (P10.1) to GGTT glyph region
- Unit-tested for allocation/free patterns + fragmentation behavior with realistic tile-churn profile
- **Deliverable:** slab serves thousands of alloc/free cycles without leak; glyph atlas lives in GGTT

### P10.5 — Tile rasterization + composition layers (1.5 weeks)  🟡 first-pass shipped

First-pass delivery (v0.57.0–.2): CpuRasterizer, render walker,
full-window back buffer (not yet tile-subdivided), persistent
overlay hook in shade, grid-aware placement on the focused window's
content rect. Files-stub visibly renders with real Inter text.

Deferred to a dedicated window-integration milestone + P10.6:
- Tile subdivision (512×512 tiles instead of one W×H buffer)
- Composition layers for opacity / transition / blur
- BCS batched blit of dirty targets
- `classify.rs` for composition-boundary detection

Original P10.5 scope:
- `kernel/src/shade/widgets/tile.rs` — tile grid per window, `TileId`, coord math
- `kernel/src/shade/widgets/classify.rs` — detect composition boundaries (opacity<1, transition in-flight, `.blur`/`.shadow`/`.effect`, Canvas, Popover/Tooltip/Menu)
- `kernel/src/shade/widgets/render.rs` — dirty-tile scheduler, per-tile raster task dispatch
- `CpuRasterizer` implements `Rasterizer` trait — wraps `gui/render.rs` primitives + fontdue glyph cache, writes into `RasterTarget` (tile or comp layer)
- Tile raster task: enumerate nodes whose rect intersects tile bounds, call rasterizer for each
- Composition-layer raster task: render single node's sub-tree into its own target
- BCS batched blit of all dirty targets in one ring submission
- **Real Inter Variable text rendered from first run** (no placeholder-rect stage)
- **Deliverable:** static file-browser tree renders in a window with actual Inter text + real rects/icons; serial debug can dump tile+layer list per commit

### P10.5b — Widget windows first-class in shade (milestone, not in original spec)

The P10.5 stopgap paints widget pixels as an overlay anchored to the
focused terminal's content rect. That works for one-shot demos but
breaks under real usage — focus-switch, workspace-switch, or two
widget apps side by side all fail. The target behaviour:

**Widget-apps behave like `loop` terminals** — they claim their own
slot in the tiling grid, can be Mod+Arrow-moved, Mod+Shift-dragged,
swapped, resized, focused, workspace-assigned, and close on
`Mod+Shift+Q` like any other window. Rounded corners and borders
come from the same shade chrome.

Scope:
- New `WindowKind` on shade::Window: `Terminal { idx }` vs `Widget { scene_id }`
- `shade::create_widget_window(title)` — enters the tiling grid via `retile()` like terminal windows do today
- Per-window scene storage (`BTreeMap<WindowId, WidgetScene>`) replacing the single global `ACTIVE_SCENE`
- `scene_commit` looks up the caller's window via `HostState.widget_window_id` (set when the app launches) and renders into that window's content rect
- `shade::compositor::render_window` gets a widget branch that blits from the per-window scene buffer
- Focus, workspace switch, move, resize, close all handled by existing shade paths — widget windows look identical to terminals from shade's perspective, only the content source differs
- Rounded corners + borders applied by shade chrome (already works, just need `WindowKind::Widget` to use the same border render)

Deliverable: `files-stub` opens into its own tiling slot next to
existing loops, can be moved/swapped/closed like a terminal, content
stays rendered across all shade redraws without the current overlay
hook.

### P10.6 — Diff + cache (4–5 days)
- Node ID + content hash
- Per-app `AppCache { tiles: HashMap<TileId, TileTexture>, layers: HashMap<NodeId, LayerTexture> }` survives across commits
- Diff pass produces dirty tile set + dirty/new/evicted layer set
- Skip tiles whose intersecting nodes all have unchanged hashes
- **Deliverable:** typing in Input marks exactly 1 tile dirty; hover on a `.transition` button re-rasters only its composition layer (verifiable via debug overlay showing dirty regions per frame)

### P10.7 — Event routing (3 days)
- Mouse: hit-test laid-out tree, find topmost `on_click` widget
- Keyboard: focus stack, Tab navigation
- `npk_event_poll` / `npk_event_wait` host fns
- **Deliverable:** clicking a button fires the action in the app

### P10.8 — Animation (1 week)
- Spring physics + linear timing in compositor, fixed-point Q16.16
- Self-scheduling 60Hz tick while active
- Interpolate `background`, `opacity`, `padding`, position deltas
- **Deliverable:** hover state on buttons fades smoothly, deterministic

### P10.9 — Icon atlas (3 days)
- `build.rs` rasterizes curated Phosphor SVGs at compile time
- 5 size variants (16/24/32/48/64), alpha-only, packed
- `IconId` enum populated, atlas embedded as `static`
- Atlas uploaded to GGTT at boot
- **Deliverable:** file-browser shows real icons in correct theme color

### P10.10 — Canvas (4–5 days)
- `npk_canvas_commit` host fn
- Size caps enforced (4K × 4K, 64 MB total per app)
- `CANVAS` capability check
- BGRA copy from WASM heap → layer texture in slab
- **Deliverable:** image-viewer stub displays a PNG decoded in the app

### P10.11 — File browser app (1 week)
- `tools/wasm/files/` — real app, walks npkFS, opens via intent
- **Deliverable:** working file browser, premium feel, no `CANVAS` cap needed

**Total: ~6–7 weeks for a polished v1 incl. file browser + Canvas. Real Inter Variable typography from P10.5 onward — no "placeholder text" phase.**

### Shipped since last milestone edit (`v0.75.x`)

Feature work beyond the numbered phases above:

- **SDK `style` module** — `Radius`, `Spacing`, `Padding`, `Elevation`
  enums (append-only, `u8`/`u16` discriminants). Apps reference
  design tokens by name; `style.rs` is the single tuning knob.
- **SDK `prefab` module — the cookbook** — `panel`, `searchbar`,
  `list_row`, `footer`, `badge`, `scroll_list`, `empty_state`,
  `title_bar`, `muted`, `body`. Apps assemble screens from these and
  never touch raw `Modifier::Padding(10)` patterns. Future AI
  app-generation reads the prefab signatures as its API.
- **`AppMeta` — per-app icon + display name + description** — shipped
  as a `.npk.app_meta` WASM custom section, generated by `build.rs`
  via `nopeek_widgets::app_meta::encode`. drun reads each module's
  section at startup (parses with a built-in LEB128 + section-walker
  — ~50 LoC in drun) so the kernel owns no meta cache. Single source
  of truth = the wasm binary.
- **`Modifier::Tint(Token)`** — paints Icons in a given token colour
  instead of the default `OnSurface`. Lets prefab select-row icons
  swap to accent without widgets touching RGB.
- **Two-theme palette (dark / light / auto)** — curated surface +
  border + text tables per mode, only `Accent` / `AccentMuted` /
  `OnAccent` derive from the wallpaper. Accent gets a contrast
  adjustment against the theme surface so icons stay visible on
  any wallpaper. `theme dark|light|auto` intent switches modes;
  `auto` picks per wallpaper luminance.
- **16×16 centered subpixel AA** — all rounded-rect sites
  (`gui/render.rs` + `shade/widgets/raster/cpu.rs`) use scale ×32
  with sample offsets `2*sx - 15` → 256 coverage levels, symmetric
  around the pixel centre. Eliminated visible alpha-plateaus on the
  inner content fill.
- **drun v0.5.x** — live-search (`KeyCode::Char`), hover+click via
  `OnHover` + `OnClick`, `ArrowRight` indicator on the selected row,
  row windowing (client-side paging for lists longer than the
  visible area), Md padding so hover chips breathe, window sized to
  fit 6 rows with sticky footer.
- **Aurora retired** — `gui/background.rs` lost its whole procedural
  gradient + cache system (~250 LoC). Default background is a flat
  grey (`#181820`) when no wallpaper is set. All pixel generation
  now lives in `wallpaper.wasm`.
- **Wallpaper generators in the WASM module** — `wallpaper.wasm` 0.4
  dispatches on target prefix: `@solid:`, `@gradient2:`, `@gradient4:`,
  `@pattern:` (dots/stripes/checker/grid/noise) plus the legacy
  `@demos:`. Kernel `intent::wallpaper` only parses the CLI, builds
  the target string, and hands it to the module — no pixel math in
  the kernel.
- **npkFS hardening** — 6 distinct write-path bugs identified + fixed
  during Phase 10 stress-testing (mostly via drun/wallpaper
  generation loops that exercise large-file delete+store):
  1. `btree::remove_from_parent` ignored `child_idx == n` (rightmost
     child) — dangling `next_leaf` pointer + orphan subtree on every
     empty-rightmost-leaf removal. (`v0.73.2`)
  2. `blkdev::discard_blocks` omitted `PARTITION_OFFSET` — every
     `flush_trims()` TRIMed ESP blocks (GRUB + `kernel.bin`).
     (`v0.73.1`)
  3. `npkfs::delete` freed indirect chain blocks via
     `free_indirect_chain` **before** journal commit — reallocated
     blocks collided with still-referenced btree entries.
     (`v0.73.0`, refined in `0.73.3`)
  4. `npkfs::store` leaked allocated extents + indirect chain on
     `btree::insert` failure (e.g. duplicate key). (`v0.73.0`)
  5. `npkfs::delete` did not record indirect chain block addresses in
     the journal — crash between Phase 3/4 leaked them forever.
     (`v0.73.3`)
  6. `cache.invalidate(ext.start_block)` in Phase 4 only touched the
     first block of each extent; rest of a 8000-block wallpaper
     extent stayed valid in the 64-slot LRU. (`v0.73.3`)
- **App metadata caching removed from kernel** — `refresh_app_metas`,
  `cache_app_meta`, `sys/meta/<name>` path, `npk_app_meta` host fn,
  `wasm_meta.rs`: all deleted. drun reads the WASM directly via
  `npk_fetch("sys/wasm/<name>")` + inline section parser.

### Next up

- **P10.11 file browser** (Thunar-clone) — new session. Name TBD.
  Depends on: `toolbar`, `sidebar` with sections, `grid_item` / grid
  layout, `breadcrumb`, `icon_button`, `nav_row` prefabs (all
  additive to the prefab cookbook).
- **drun polish** — mouse-wheel scroll when MAX_VISIBLE exceeded,
  visual fine-tuning after file browser clarifies the shared style
  vocabulary.
- **P10.10 Canvas** — on hold until a concrete consumer (image
  viewer, chart, …) asks for it.

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
| Font backend | `fontdue` + **Inter Variable** (OFL) for UI; Spleen kept for terminals | Modern UI typography needs vector fonts. Variable font = one file, all weights via single axis, smoother interpolation, ~800KB vs ~600KB for Regular+Bold pair. Zero extra code in fontdue. |
| Font phase | Bootstrap in **P10.1**, rasterize in P10.5 — not deferred | Layout needs real metrics; a "placeholder-rect" stage would require rework. Cost: +3 days in P10.1. |
| OpenType features | `tnum` + `kern` enabled by default, `liga` deferred to v2 | Tabular numerals eliminate number jitter in lists/clocks — biggest "premium feel" upgrade for minimal effort. Ligatures and complex-script shaping require `rustybuzz`, defer to v2 (no ABI change). |
| Font metrics | Read from Inter's tables (`hhea`, `hmtx`, `OS/2`) — no hardcoded line-height | Vertical rhythm correct per weight/size; mixed-style rows align on baseline. |
| Font storage | npkFS, BLAKE3-verified | Matches content-addressing principle; OTA-updatable without kernel rebuild |
| Icon source | Phosphor (MIT), pre-rasterized at build time, alpha-only | Runtime SVG parsing is fragile or huge; alpha + theme token gives free re-coloring |
| Icon sizes | 16/24/32/48/64 px logical (5 variants) | Covers 1× and 2× HiDPI cleanly |
| Serializer | Postcard with version byte | Smallest, no_std, Rust-native; version byte enables forward evolution |
| GGTT allocator | Slab with fixed buckets (1KB–4MB), LRU eviction | Bump allocator fragments under churn; slab is bounded |
| Canvas in v1 | Yes, but tightly scoped: leaf-only, size-capped, separate capability | Need it for image viewer demo; constraints prevent GDI-style drift |
| Cache scope | Per-app, no cross-app sharing | Side-channel concerns + eviction complexity outweigh ~100KB savings |
| Capability split | `RENDER` and `CANVAS` separate | Least privilege: file-browser doesn't need to draw pixels |
| Animation math | Fixed-point Q16.16, tick-based | Float non-determinism across cores breaks visual consistency |
| Enum variant ordering | `#[non_exhaustive]` + append-only on every ABI enum | Postcard serializes by position — inserting breaks every persisted tree |
| Reserved widget slots | `Popover`, `Tooltip`, `Menu` declared in v1 | Out-of-window rendering cannot be retrofitted without wire-version bump |
| Reserved modifier slots | `.blur`, `.shadow`, `.effect`, `.role` in v1 | GPU-dependent effects and a11y added later with zero ABI churn |
| Rasterizer abstraction | `trait Rasterizer`, CPU in v1, GPU in v2 | Single swap point for HW acceleration; no call site locked to CPU |
| Threading split | Raster on worker cores, BCS submit on Core 0 | Phase 9 constraint: Core 0 dispatches events, never blocks >100 μs |
| A11y role reservation | `Role` byte per node from v1, consumed later | AppKit precedent: retrofitting a11y is catastrophic; ~200 bytes/tree is nothing |
| Raster granularity | **Tile-based (512×512 actual px, 1 MB) + composition layers at boundaries** — not per-leaf | Per-leaf textures die under real workloads: slab fragmentation from thousands of 1-KB allocations, per-blit BCS overhead. Tiles are geometry-driven (stable across tree rebuilds), map cleanly to the 1-MB slab bucket, enable parallel worker-core rasterization (one tile = one task). Composition layers handle animated / semi-transparent / overlay widgets without dirtying tiles every frame. Same model as Blink/WebKit post-~2013. |
| Composition boundaries | opacity<1, transition in-flight, blur/shadow/effect, Canvas, Popover/Tooltip/Menu | Narrowly defined; everything else goes into tiles. Prevents boundary-inflation that wasted memory in early Chromium. |

---

## Next vocab pass — Tailwind-style modifiers + container queries

**Motivation.** Today's apps (drun, loft) compose UI from raw `Padding`/`Background`/`Border`/`Tint` modifiers. Each screen takes 20+ iterations to look right because the SDK has no opinion on spacing, elevation, hover/focus states, or responsive behaviour. For Phase 11 (AI generates apps), this is fatal: an LLM cannot learn our bespoke combinations from zero examples.

**Direction.** Mirror Tailwind's vocabulary as a typed Rust API — same words AI already knows from millions of training examples, but compile-checked and capability-aware. `padding`, `rounded`, `shadow`, `bg`, `hover:scale`, `transition`, `focus`, `gap` become first-class modifiers. Pico's lesson layers on top: `prefab::card`/`button`/`input` ship sensible defaults so the common case is one call, not a chain.

**Why not embed CSS / HTML.** Runtime CSS parser is 50–200 KB of WASM bloat per app, untyped (silent property drops are the worst UX failure mode), and the cascade model collides with our per-widget capability tree. Tailwind-style typed modifiers give the same look-and-feel sweet spot AIs target without parsing strings or shipping a layout engine. If parser-style ergonomics are ever wanted, a build-time `css! {}` proc-macro can desugar to typed calls — no runtime cost, no ABI change.

### Token scale

```
Spacing   xs(4) sm(8) md(12) lg(16) xl(24) 2xl(32) 3xl(48)
Radius    none sm(4) md(8) lg(12) xl(16) full(9999)
Shadow    none sm md lg xl                     // semantic Elevation, not free values
Motion    instant(0) quick(120ms) normal(200ms) slow(400ms)
Density   Compact Regular Spacious             // compositor-set per window
```

Spacing scale is Tailwind-compatible (4-px steps, sm/md/lg). Existing `Token` palette (Surface/Border/Text/Accent + Tint) stays as-is — append-only.

### Modifier additions (append-only on existing enum)

```rust
Modifier::Gap(Spacing)                    // for Column/Row gap; replaces Padding-as-spacer
Modifier::Rounded(Radius)
Modifier::Shadow(Elevation)
Modifier::Bg(Token)
Modifier::Border(Token, u16)
Modifier::Scale(u16)                      // Q8.8: 256 = 1.0
Modifier::MinWidth(u16)
Modifier::MaxWidth(u16)
Modifier::Flex(u8)

Modifier::Hover(Vec<Modifier>)            // pseudo-state: nested modifiers apply on hover
Modifier::Focus(Vec<Modifier>)
Modifier::Active(Vec<Modifier>)
Modifier::Disabled(Vec<Modifier>)

Modifier::Transition(Motion)              // smooths state-driven prop changes
Modifier::WhenDensity(Density, Vec<Modifier>)   // container query
```

Pseudo-states carry **modifier lists**, not separate variants — compositor applies the inner list when the state matches. Tree stays static (no re-commit on hover), wire format stays compact.

### Density as container query

Compositor classifies each window:
- `Compact` < 600 px width
- `Regular` 600–1200 px
- `Spacious` > 1200 px

Thresholds live **once in the compositor**, app sees only the enum. `WhenDensity(Compact, [...])` matches and merges the inner modifiers. App-side hints:

```c
i32 npk_window_set_min_size(u32 w, u32 h);     // compositor won't tile narrower
i32 npk_window_set_ideal_size(u32 w, u32 h);   // initial-tile hint
```

### Archetype prefabs (phase 1 set)

```
prefab::card(content, opts)                    // Surface + Padding + Shadow + Rounded
prefab::button(label, style, action)           // Primary | Secondary | Ghost | Destructive
prefab::input(buf, kind)                       // Text | Search | Password
prefab::list_row(icon, title, sub, action)
prefab::toolbar(items)                         // Compact: icon-only, Regular+: with labels
prefab::sidebar(sections)                      // Compact: icon-rail or hidden
prefab::grid(items, min_item_width)            // auto-fit columns
prefab::dialog(title, body, actions)
```

Each prefab encapsulates responsive behaviour internally. App-dev calls `prefab::sidebar(...)` and gets the right behaviour in every density class without touching `WhenDensity` directly.

### drun migrated (sketch)

```rust
prefab::dialog()
    .title(Icon::MagnifyingGlass, "Run app")
    .body(
        Column::new().gap(Spacing::sm).children([
            prefab::input(&query, InputKind::Search).autofocus(),
            prefab::scroll_list(matches.iter().map(|m| {
                prefab::list_row(m.icon, &m.title, &m.subtitle, Action::Launch(m.id))
                    .hover(|h| h.bg(Token::SurfaceHover).scale(1.01))
                    .transition(Motion::quick)
            }))
        ])
    )
    .footer_hint("↑↓ select · Enter launch · Esc close")
    .min_size(380, 480)
```

Equivalent today is ~3× the lines and a maze of `Padding(8)` / `Background(Token::SurfaceMuted)` / `Border(...)` / hand-built hover via re-commit. Migration is mostly mechanical translation, not redesign.

### ABI implications

- `Modifier` enum stays append-only; new variants require no wire-version bump (still `0x01`).
- New token enums (`Spacing`, `Radius`, `Elevation`, `Motion`, `Density`) all `#[non_exhaustive]`, `#[repr(u8)]`, frozen on first ship.
- `Vec<Modifier>` inside `Hover`/`Focus`/`Active`/`Disabled`/`WhenDensity` — postcard handles natively.
- Compositor render contract: unknown modifiers are **logged + ignored**, not rejected. (Decision below.) Old kernel + new SDK should produce a valid (if visually flatter) render.

### Open question: unknown-modifier handling

Today: `npk_scene_commit` rejects unknown variants strictly. Strict is good for catching SDK typos, bad for forward-compat (old kernel + new SDK = app refuses to start).

**Proposal:** ignore-with-warning. Matches CSS behaviour ("unknown property → drop"), decouples OTA upgrade order (apps can target newer modifiers as soon as their kernel catches up, no hard pinning), and pseudo-states stay safe (Hover ignored = visual loss, not crash). To be confirmed before implementation.

### Implementation order

1. **SDK token enums + modifier additions** — 1 day, mechanical
2. **Compositor pass: shadow / scale / opacity / transition** — 2–3 days, the visible jump
3. **Pseudo-state engine** — Hover/Focus/Active routing + render — 1–2 days
4. **Density classifier + WhenDensity resolver** — 1 day
5. **Archetype prefabs** — Card/Button/Input/ListRow/Toolbar/Sidebar/Grid/Dialog — 2–3 days
6. **Migrate drun** — 1 day, sanity-check the vocabulary
7. **Migrate loft** — 1 day
8. **Subset documentation** — what's in, what isn't — 0.5 day

~2 weeks to drun + loft running on the new vocabulary. Biggest risk is step 3 (touching mouse/keyboard routing).

### Why this is the right time

drun + loft are the only widget apps. Migration cost is bounded (~2 days total). Every app written *after* this lands inherits the polish for free. Every app written *before* this lands will be rewritten. The asymmetry only gets worse with each new app.

---

*Last updated after P10.0 design pass + critical review (enum ordering, reserved slots, Rasterizer trait, threading, A11y role, tile-based rasterization). Implementation starts at P10.0 (ABI freeze).*
