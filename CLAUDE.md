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

- Language: Rust (no_std, nightly, edition 2024)
- Target: `x86_64-nopeek` — our own spec in `targets/`, = bare metal WITH
  SSE/AVX2/AES-NI. Do not go back to overriding features on
  `x86_64-unknown-none`; that contradicts its softfloat ABI and rustc is
  turning it into a hard error.
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
./build.sh usb-full /dev/sdX  # USB stick + LibreWolf bundle (~290 MB,
                              # browser ready on first boot, no OTA needed)
./build.sh qemu-installer-full  # QEMU installer test with bundle
```

## Current Status

**Stand 2026-08-15 · kernel v0.267.7 · beak 0.24.1 · wifi_ax200 0.54.2 ·
wifid 0.3.0**

Zwei Fäden laufen parallel.

**`beak`**, der eigene Browser: Stage 0 (HTML, CSS, Layout, Paint — noch kein
JavaScript) läuft auf Hardware; die Arbeit dreht sich um Konformität und um
echte Seiten (POST, Cookies, Zeichensätze, TLS-Wurzelspeicher). Die gemessene
WPT-Zahl steht in `docs/spec/CONFORMANCE.md` und nirgends sonst.

**WLAN (AX200)**: lädt Wikipedia über Funk. Das Intent **`wlan`** ist das
Werkzeug dafür — Kernel-Sicht plus ein Klartext-Report, den der Treiber selbst
veröffentlicht (Rate, Retries, Airtime, 4-Way-Sprosse, Ring-Zustand) plus der
wifid-Log. Offen: Stabilität über die Zeit, und Durchsatz — gemessen 16 Mbit/s
bei 130 Mbit PHY, der Hebel ist A-MPDU. Fahrplan in
`memory/project_wifi_ax200.md`.

Alles darunter — Kernel, npkFS, Netz, Compositor, Panels, Apps, MicroVM —
ist gebaut und in Betrieb. Überblick: `README.md`.

Wo der Stand wirklich steht:

- `memory/MEMORY.md` — Index auf die Themen-Files, wird laufend gepflegt
- `git log` — die harte Wahrheit
- `docs/spec/` — lebende Verträge · `docs/plan/` — offene Papiere ·
  `docs/archive/` — erledigt/überholt

> Dieser Abschnitt bleibt **kurz**. Session-Verlauf gehört ins Memory, nicht
> hierher; der alte Verlauf liegt in `docs/archive/CHANGELOG_2026.md`.

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
# bump the version, then sync the lock — builds run --locked, so a stale
# Cargo.lock aborts the release instead of silently re-resolving:
cargo update --offline -p nopeekos-kernel

./build.sh build      # verify it builds
git commit -m "..."   # source change
./build.sh release    # target/ → signed release/
git add release/ && git commit -m "release: sign + publish vX.Y.Z"
git push
```

`release` does NOT compile WASM. A changed module must be built and staged
first (`tools/stage-module.sh <mod>`, which also writes `.version`);
`aml` and `wifid` live one level deeper than the script expects and are
staged by hand.

USB reinstall pulls `target/` directly and bypasses this — that's why
USB-installed builds appeared to work while OTA kept downgrading.

## Security Checkpoint

Before every commit:
"Can a WASM module escape its sandbox through this change?"
If the answer isn't clearly "No" → don't commit.
