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

- **Phase:** 10 (Widget API & GUI Apps) — kernel `v0.79.0`, sdk `0.4.0`,
  drun `0.5.6`, loft `0.1.6`.
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
  - **Two-theme palette** (dark/light/auto) with wallpaper-driven accent +
    16×16 subpixel AA on rounded rects (kernel-side polish from 0.75.x).
  - npkFS hardened — 6 write-path bugs fixed in 0.73.x.
  - **Next:** static visual effects (Shadow / Transition / Scale via
    compositing-layer pass), tile subdivision + full diff cache,
    Widget::Input self-editing, Canvas (P10.10).
- **Parallel track:** Phase 9 SMP/event-driven (WiFi driver, per-core timer)
- **Completed features + full roadmap:** see `README.md`
- **Phase 10 detail spec + progress:** see `PHASE10_WIDGETS.md`
- **Vocab-v2 reference (for AI / app devs):** see `WIDGET_VOCAB.md`
- **Active work / blockers:** see `memory/project_wifi_current.md`

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
