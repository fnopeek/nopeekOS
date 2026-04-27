# CLAUDE.md – nopeekOS Development Guide

## What is nopeekOS?

An AI-native operating system, rethought from scratch.
Not a Unix clone. Not POSIX. No legacy.

See README.md for the full vision and phase planning.

## Architecture Principles (DO NOT violate)

1. **Capabilities, not Permissions** – No chmod, no ACLs, no root
2. **Intents, not Commands** – Express intention, not instructions
3. **Content-addressed, not path-addressed** – No filesystem tree
4. **Runtime-generated, not pre-installed** – Tools built on demand
5. **Formally bounded** – WASM sandbox as trust boundary

## Code Rules

- Language: Rust (no_std, nightly)
- Target: x86_64-unknown-none
- No POSIX, no libc, no std
- Every resource is capability-gated
- Panic = Kernel Panic = Halt (no recovery in Phase 1)
- All `unsafe` blocks MUST have a SAFETY comment
- Serial is primary I/O, not VGA
- Comments in English, minimal
- Hardware drivers: follow Linux source 1:1 (see memory/feedback_linux_strict.md)

## Build & Run

```bash
./build.sh build        # Compile only
./build.sh qemu         # Build + QEMU (development)
./build.sh debug        # Build + QEMU with GDB stub
./build.sh release      # Build + sign (ECDSA P-384) → release/ for OTA
./build.sh vbox         # Build + VirtualBox (demo)
./build.sh vbox-clean   # Remove VirtualBox VM
./build.sh installer    # Two-pass installer build (bundled assets)
./build.sh usb /dev/sdX # Build installer + flash USB stick
```

## Current Status

- **Phase:** 10 (Widget API & GUI Apps) — kernel `v0.80.1`, sdk `0.5.1`,
  drun `0.5.10`, loft `0.1.10`.
  - **Vocab v2 shipped** (Tailwind-style modifier set + pseudo-state engine):
    Hover / Focus / Active / Disabled / WhenDensity, Rounded, MinWidth /
    MaxWidth, Scale (Q8.8 reserved), 9 new Modifier variants append-only
    to the Wire ABI (still WIRE_VERSION 0x01).
    Compositor tracks per-window hover_path / focus_path / active_path,
    re-rasterizes only when has_pseudo + path actually changed.
    Tab / Shift+Tab navigation walks focusable widgets in document order.
    `WIDGET_VOCAB.md` at the repo root is the AI / app-dev reference.
  - **Apps complete**: drun (Mod+D launcher, modal overlay), loft (file
    browser — Thunar-clone, sidebar + breadcrumb + grid + toolbar). Both
    use the v2 prefab cookbook (card, button, input, dialog, sidebar_pane,
    list_row, nav_row, grid_item, icon_button, …). Hover/Focus borders
    visible across all interactive widgets.
  - **Mockup-grade polish round 1–3 shipped (0.79.4 → 0.80.1):** SDF
    rounded corners (Hyprland-style concentric arc geometry), widget
    chrome `paint_content=false` so the inner-fringe AA blends against
    the widget background instead of `win.bg_color`,
    `place_axis` layout-rect fix (Background/Border now paint on the
    container rect — children sit inside padding, no chrome-on-content
    bleed), `TextStyle::Heading` appended (18 px, ABI variant 5),
    card-style selection (SurfaceElevated + Accent border instead of
    AccentMuted fill), `prefab::panel` 4 px inset, footer + search
    symmetric vertical breathing room, `suppress_hover()` on keyboard
    dispatch so arrow-key nav owns the highlight until the mouse moves
    again.
  - **Two-theme palette** (dark/light/auto) with wallpaper-driven accent +
    16×16 subpixel AA on rounded rects (kernel-side polish from 0.75.x).
  - npkFS hardened — 6 write-path bugs fixed in 0.73.x.
  - **Next (priority order):**
    1. **`Widget::Input` self-editing** — compositor-side cursor +
       key-routing-to-focused-input + Submit-on-Enter (~2 d). Unblocks
       proper drun search UX, kills the per-app `read_line` plumbing.
    2. **Tile subdivision + full diff cache** — 512×512 grid + per-tile
       content-hash, so hover/key changes only re-rasterize the dirty
       tiles instead of the whole window (~3–5 d).
    3. **Static visual effects** (`Shadow` / `Transition` / `Scale`
       outside pseudo-states) — needs a compositing-layer pass
       (sub-tree → off-screen layer texture → blit with transform). ~1
       Woche, größerer Brocken.
    4. **P10.10 Canvas escape hatch** — `npk_canvas_commit` + `CANVAS`
       cap, on hold until ein konkreter Consumer (image viewer, chart)
       danach fragt.
- **Parallel track:** Phase 9 SMP/event-driven (WiFi driver, per-core timer)
- **Completed features + full roadmap:** see `README.md`
- **Phase 10 detail spec + progress:** see `PHASE10_WIDGETS.md`
- **Vocab-v2 reference (for AI / app devs):** see `WIDGET_VOCAB.md`
- **Active work / blockers:** see `memory/project_wifi_current.md`
- **Acknowledged tech debt:** `NPKFS_V2.md` — v1 npkFS uses path-
  as-key + `.dir` markers; v2 redesigns to content-addressed tree
  objects (Git-style). Targeted Phase 11.5. Until then, follow
  the constraints in that doc's "Hooks for now" section: don't
  add scan-the-world host fns, don't pile on `.dir`-marker logic,
  apps treat paths as opaque strings.

## Commit-Message Convention (since v0.54.x)

First line encodes which OTA path the change needs, so users know
whether a `update` is enough or modules must be `install`-ed too:

- `kernel-only:` — `update` suffices, no module rebuild
- `module <name>:` — only `install <name>` required
- `abi+kernel:` — kernel + all SDK-using apps, coordinated release
- `kernel+module <name>:` — both, because they belong together
- **Known bug:** `run wifi` on worker core crashes; `driver wifi` on Core 0 works
  (MMIO `map_page` conflict with 1GB huge pages).

## Security Checkpoint

Before every commit:
"Can a WASM module escape its sandbox through this change?"
If the answer isn't clearly "No" → don't commit.
