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

## Build & Run

```bash
./build.sh build        # Compile only
./build.sh qemu         # Build + QEMU (development)
./build.sh debug        # Build + QEMU with GDB stub
./build.sh vbox         # Build + VirtualBox (demo)
./build.sh vbox-clean   # Remove VirtualBox VM
```

## Current Phase: 1 (Bare Metal Boot)

Focus: Kernel boots, serial I/O works, intent loop runs.
Completed: IDT+PIC, physical memory manager, heap allocator, SMP (4 cores),
  xHCI (keyboard+mouse), NVMe, Intel Xe GPU (4K@60Hz native modesetting),
  shade compositor (windows, drag, resize), WASM sandbox, npkFS, OTA updates,
  network stack (TCP/TLS 1.3), login screen, double-buffer framebuffer,
  BCS blitter engine (GPU blit via Gen 12 ExecList/ELSQ — zero-CPU compositing).
In progress: GPU-composited cursor (cursor in shadow buffer, single GPU blit path).
Next: Phase 2 (Capability enforcement, audit log).

## Security Checkpoint

Before every commit:
"Can a WASM module escape its sandbox through this change?"
If the answer isn't clearly "No" → don't commit.
