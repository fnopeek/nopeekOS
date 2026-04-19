# Phase 10 — Widget API & GUI Apps

**Goal:** Apps describe **what** to render (declarative widget tree). The Shade compositor owns **how** (layout, rasterization, GPU compositing, animation, theming). Apps never touch pixels for normal UI; a single `Canvas` escape hatch exists for special cases.

**Sweet spot between immediate-mode (App calls `draw_rect`) and full retained-mode (App holds scene-graph handles):** App calls `render()` whenever its state changes, builds a fresh tree as plain data, commits in one host call. Compositor diffs against previous tree and only re-rasterizes changed sub-trees.

Inspired by SwiftUI / Slint / Compose — stripped down, capability-gated, WASM-native.

---

## Why this fits nopeekOS

| Principle | Fit |
|---|---|
| Capabilities, not Permissions | Tree commit is one cap-gated host call (`npk_scene_commit`) |
| Intents, not Commands | App declares intent (`List{items, selected}`), compositor executes |
| Sandboxed | App has zero access to GPU, framebuffer, fonts |
| No legacy | No GDI handles, no X11 protocol, no CSS box model |
| Greenfield | Layer-tree is data, not API-call stream — auditable, replayable, deterministic |

---

## What stays untouched

- Shade compositor (tiling, swap-anim, mouse) — `kernel/src/shade/`
- Window management, keybindings, shadebar
- Loop / intent dispatcher (`kernel/src/intent/`)
- Existing rendering primitives (`gui/render.rs`, `gui/font.rs`) — reused, target changes from shadow to layer texture
- BCS blitter, GPU HAL, GGTT layout
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
nopeek_widgets SDK (postcard serialize)
   │
   │  ~few KB bytes
   ▼
npk_scene_commit(ptr, len, cap_id)   ◄─ host fn, capability-gated
   │
   ▼
Compositor
   ├─ Deserialize tree
   ├─ Layout pass (flexbox-lite) → assigns x/y/w/h to every node
   ├─ Diff against previous tree (structural, by widget ID + content hash)
   ├─ For each changed sub-tree:
   │     └─ Rasterize into layer texture (CPU, reuses gui/render.rs)
   │
   ▼
GPU (BCS XY_FAST_COPY_BLT)
   └─ Blit layer textures → window region in framebuffer
```

**Key property:** App-side allocations free after `commit`. Compositor owns everything that survives the call.

---

## Widget set v1

Minimal, composable, no leaf escape hatches except `Canvas`.

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

**Compound (built from leaves, may stay in SDK):**
- `Toolbar`, `StatusBar`, `Sidebar`, `List`, `Breadcrumb`, `IconGrid`

**Escape hatch:**
- `Canvas { width, height, pixels: BGRA_buffer }` — for image viewer, chart, video. Used sparingly.

**Modifiers** (chained on any widget):
- `.padding(n)`, `.margin(n)`
- `.background(Token)`, `.border(Token, width, radius)`
- `.opacity(0.0..1.0)`
- `.transition(Spring | Linear { ms })` — declares this widget should animate when its props change
- `.on_click(ActionId)`, `.on_hover(ActionId)`

---

## Theme tokens

App **never** specifies hex colors. It uses tokens; compositor resolves against active palette (extracted from wallpaper).

```rust
enum Token {
    // Surfaces
    Surface,           // window background
    SurfaceElevated,   // cards, dialogs
    SurfaceMuted,      // sidebar, secondary regions

    // Text
    OnSurface,         // primary text on Surface
    OnSurfaceMuted,    // secondary text
    OnAccent,          // text on Accent button

    // Accent
    Accent,            // primary action color
    AccentMuted,       // hover/inactive variants

    // Semantic
    Border,
    Success, Warning, Danger,
}
```

Tokens map to indices into the existing 16-color `PALETTE` (`gui/color.rs`). Theme change = repaint all layer textures with new palette, no app involvement.

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

Compositor maintains `HashMap<NodeId, LayerTexture>` — survives across commits.

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

Animation tick = compositor runs at 60Hz **only while interpolations are active**. Otherwise dirty-driven (event → render → blit → idle).

---

## Host functions (new)

```c
// Commit a widget tree. `bytes` is postcard-serialized.
// Returns 0 on success, -1 on parse error or cap denied.
i32 npk_scene_commit(const u8* bytes, u32 len, u32 cap_id);

// Read input event for this app (key, mouse, focus).
// Returns serialized event or -1 if no event pending.
i32 npk_event_poll(u8* buf, u32 buf_max);
i32 npk_event_wait(u8* buf, u32 buf_max, u32 timeout_ms);

// Optional: query current theme tokens (so app can pick icon variant).
i32 npk_theme_token(u32 token_id) -> u32 rgba;
```

Cap requirement: app needs `RENDER` capability for its window (delegated by compositor at spawn).

`npk_print` / `npk_clear` stay for terminal-style apps (`top`, REPLs).

---

## What apps explicitly do NOT get

- No `npk_draw_*` immediate-mode functions
- No font loading (system font + 4 sizes only)
- No custom shaders
- No GPU texture handles
- No window-decoration control (compositor owns chrome)
- No raw framebuffer access
- No animation scripting (declarative only)

Reduces attack surface, prevents per-app drift, makes 4K-scaling and theme-changes universal.

---

## Implementation phases

Order matters — each phase produces something runnable.

### P10.1 — SDK + serialization (1 week, no kernel changes yet)
- `tools/wasm/sdk/widgets/` — new shared crate
- Define `Widget` enum, `Modifier`, `Event`, `Action`
- Postcard serialization (already in scope, no_std)
- Unit tests round-tripping trees
- **Deliverable:** SDK compiles, tree can be serialized to bytes

### P10.2 — Compositor receiver + dummy renderer (3–5 days)
- `kernel/src/shade/widgets/mod.rs` — new module
- `npk_scene_commit` host fn — deserialize, log to serial, no render yet
- `tools/wasm/files-stub/` — dummy app, sends one tree on launch
- **Deliverable:** see deserialized tree printed on serial when app runs

### P10.3 — Layout engine (1 week)
- `kernel/src/shade/widgets/layout.rs` — flexbox-lite
- Assigns absolute x/y/w/h to every node
- Tested standalone with snapshot tests against known trees
- **Deliverable:** layout pass produces correct geometry, dumped to serial

### P10.4 — Layer texture rasterization (1 week)
- `kernel/src/shade/widgets/render.rs` — walks laid-out tree
- Reuses `gui/render.rs` primitives, target = per-node layer buffer
- Allocates layer textures in GGTT (need GGTT allocator beyond FB region)
- BCS blits layer textures into window content region
- **Deliverable:** static file-browser tree renders correctly in a window

### P10.5 — Diff + cache (4–5 days)
- Node ID + content hash
- `HashMap<NodeId, LayerTexture>` survives across commits
- Skip rasterize when hash unchanged
- **Deliverable:** typing in Input re-rasterizes only that node (verifiable via debug overlay)

### P10.6 — Event routing (3 days)
- Mouse: hit-test laid-out tree, find topmost `on_click` widget
- Keyboard: focus stack, Tab navigation
- `npk_event_poll` / `npk_event_wait` host fns
- **Deliverable:** clicking a button fires the action in the app

### P10.7 — Animation (1 week)
- Spring physics + linear timing in compositor
- Self-scheduling 60Hz tick while active
- Interpolate `background`, `opacity`, `padding`, position deltas
- **Deliverable:** hover state on buttons fades smoothly

### P10.8 — Icon atlas (3 days)
- Built-in SVG-subset → pre-rasterized at 16/24/32/48px
- Phosphor-inspired open icon set, embedded as `.rs` constants
- `Icon::Folder`, `Icon::File`, `Icon::ArrowLeft`, etc.
- **Deliverable:** file-browser shows real icons

### P10.9 — File browser app (1 week)
- `tools/wasm/files/` — real app, walks npkFS, opens via intent
- **Deliverable:** working file browser, GNOME-feel

**Total: ~5–6 weeks for a polished v1 incl. file browser.**

---

## Open questions to decide before starting

1. **System font:** stay with Spleen bitmap (3 sizes) or add a small TTF rasterizer (`fontdue` is no_std, ~2k LOC, supports Inter)? Affects "schön" achse heavily.
2. **Icon source:** hand-coded paths, SVG subset parser, or pre-rasterized PNGs in atlas?
3. **Postcard vs. CBOR vs. custom:** postcard is smallest (~half the size of CBOR), Rust-only. CBOR is wire-portable.
4. **GGTT allocator:** today only framebuffer is in GGTT. Layer textures need a real allocator — small bump+free or full slab?
5. **Canvas escape hatch:** ship in v1 or defer to v1.1? (Image viewer needs it.)
6. **Per-app vs. global widget cache:** if two windows show the same icon, share texture or duplicate?

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

*This doc is exploratory. Discuss before commit.*
