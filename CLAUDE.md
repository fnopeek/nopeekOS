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

- **Phase:** 10 (Widget API & GUI Apps) — kernel `v0.85.5`, sdk `0.6.1`,
  drun `0.6.0`, loft `0.2.1`.
- **Just shipped (2026-04-28):**
  - **npkFS v2** — content-addressed Git-style tree objects, real
    directories, walk-by-hash path resolution. Clean break, no
    migration. v1 deleted. See `NPKFS_V2.md`.
  - **HW Crypto + SSE/AVX2 bring-up** — npkFS storage on AES-256-GCM
    (AES-NI + PCLMULQDQ), BLAKE3 on AVX2 backend, NVMe queue depth
    256 + DMA pool 128, in-place AEAD decrypt. Measured: 1 MB write
    75→208 MB/s, 1 MB read 87→216 MB/s, energy/byte 5× better. CR4
    OSFXSR/OSXMMEXCPT/OSXSAVE + XSETBV in boot.s/trampoline.s before
    first Rust instruction. See `memory/project_hw_crypto.md`.
- **Resuming next (Phase 10 polish queue):**
  1. **Tile subdivision + full diff cache** — 512×512 grid + per-tile
     content-hash, so hover/key changes only re-rasterize the dirty
     tiles instead of the whole window (~3–5 d).
  2. **Static visual effects** (`Shadow` / `Transition` / `Scale`
     outside pseudo-states) — needs a compositing-layer pass
     (sub-tree → off-screen layer texture → blit with transform). ~1
     Woche, größerer Brocken.
  3. **P10.10 Canvas escape hatch** — `npk_canvas_commit` + `CANVAS`
     cap, on hold until ein konkreter Consumer (image viewer, chart)
     danach fragt.
  4. **Loft polish round 4** — dropdown menus once `Widget::Popover`
     lands (Phase 11+), `.trash`-click crash investigation.
- **Already in-place from earlier rounds (kept here as quick reference):**
  - Vocab v2 shipped (9 Modifier variants — Hover/Focus/Active/Disabled/
    WhenDensity/Rounded/MinWidth/MaxWidth/Scale, Wire ABI 0x01).
  - Apps complete: drun (Mod+D launcher), loft (file browser).
    Both on prefab cookbook (card/button/input/dialog/sidebar_pane/…).
  - SDF rounded corners (Hyprland-style concentric two-arc geometry).
  - `TextStyle::Heading` (ABI variant 5, 18 px regular).
  - `Widget::Input` self-editing — compositor owns cursor + key routing.
  - Layout leaf-padding (Text/Icon/Input/Checkbox/Canvas).
  - Two-theme palette (dark/light/auto, wallpaper-derived accent).
- **Parallel track:** Phase 9 SMP/event-driven (WiFi driver, per-core timer).
- **Completed features + full roadmap:** see `README.md`.
- **Phase 10 detail spec + progress:** see `PHASE10_WIDGETS.md`.
- **Vocab-v2 reference (for AI / app devs):** see `WIDGET_VOCAB.md`.
- **Active work / blockers:** see `memory/project_wifi_current.md`.

## Commit-Message Convention (since v0.54.x)

First line encodes which OTA path the change needs, so users know
whether a `update` is enough or modules must be `install`-ed too:

- `kernel-only:` — `update` suffices, no module rebuild
- `module <name>:` — only `install <name>` required
- `abi+kernel:` — kernel + all SDK-using apps, coordinated release
- `kernel+module <name>:` — both, because they belong together
- **Known bug:** `run wifi` on worker core crashes; `driver wifi` on Core 0 works
  (MMIO `map_page` conflict with 1GB huge pages).

## Release-Flow Plumbing (mandatory)

`./build.sh release` regenerates `release/kernel.bin` + `release/manifest`
+ all `release/modules/*.sig` with the ECDSA P-384 update key. Skipping
this step means OTA users keep getting the LAST signed release — every
`update` is a silent downgrade to whatever was last in `release/`.
Bitter lesson from v0.85.0–0.85.5: pushed source, forgot release-build,
user's `update` rolled back to v0.84.3 every time → consistent
"wrong passphrase" lockout because v0.84.3 ChaCha20 couldn't decrypt
v0.85.x AES-GCM keycheck.

Sequence for any kernel/module change:

```
./build.sh build      # verify it builds
git commit -m "..."   # source change
./build.sh release    # target/ → signed release/
git add release/ && git commit -m "release: sign + publish vX.Y.Z"
git push
```

USB reinstall pulls `target/` directly and bypasses this — that's why
USB-installed builds appeared to work while OTA kept downgrading.

## Security Checkpoint

Before every commit:
"Can a WASM module escape its sandbox through this change?"
If the answer isn't clearly "No" → don't commit.
