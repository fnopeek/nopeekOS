# Return thunks and retpolines: dropping the IBPB per VM exit on Zen 2

Status: paper, nothing built. Decide before starting.

## Why

On Zen 2 (IdeaPad, Ryzen 5 5500U, family 0x17) every VM exit runs an IBPB,
because the host has no other guest→host branch-predictor protection there
(`spec_mitigations.rs`). Measured with `npkbench_vm`, kernel 0.595.0:

| IdeaPad | IBPB on exit | IBPB off (measurement only) |
|---|---|---|
| VM exit (`cpuid`) cpu0 / cpu7 | 1718 / 1379 ns | 803 / 745 ns |
| unmap, others running | 14.4 µs | 8.5 µs |
| mprotect pair, others running | 16.9 µs | 10.7 µs |

The IBPB is about half of every exit. Running without it is not an option:
it is the only thing that stops a guest from steering the host's indirect
branches and returns (Spectre v2, retbleed, SRSO).

What we run is a configuration Linux supports (`retbleed=ibpb`,
`srso=ibpb-vmexit`). It is not Linux's default on Zen 2.

## What Linux does on Zen 2 by default

Linux 6.18.55, `arch/x86/kernel/cpu/bugs.c`:

- spectre_v2: retpolines (no eIBRS/AutoIBRS on Zen 2). Every indirect
  `call`/`jmp` in the kernel goes through `__x86_indirect_thunk_<reg>`.
- retbleed: `RETBLEED_MITIGATION_UNRET` (1486-1497): `X86_FEATURE_RETHUNK`
  and `X86_FEATURE_UNRET` — every `ret` becomes `jmp __x86_return_thunk`,
  and the return predictor is untrained on kernel entry
  (`entry_untrain_ret`, `arch/x86/lib/retpoline.S:387`). STIBP stays on
  with SMT.
- SRSO: `SRSO_MITIGATION_SAFE_RET` (3193-3210): the return thunk becomes
  `srso_return_thunk` (`retpoline.S:279`), untraining is
  `srso_untrain_ret` (251). Family 0x19 uses the `srso_alias_*` variants.
  With `SRSO_USER_KERNEL_NO` Linux falls back to IBPB on VM exit (3135).
- No IBPB per VM exit in this mode.

To verify before building: where exactly the untrain runs on the VM-exit
path in 6.18 — `kvm/svm/vmenter.S` itself has no `UNTRAIN_RET` any more, so
it is reached through the return thunk and the entry paths; the port has to
follow the same calls, not a guess about them.

## What it takes here

1. **Kernel build.** `rustc` nightly (1.99) has what is needed:
   `-Zfunction-return=thunk-extern` (every `ret` → `jmp __x86_return_thunk`)
   and `-Zretpoline-external-thunk` (indirect `call`/`jmp` →
   `__x86_indirect_thunk_<reg>`). It goes into the kernel's rustflags in
   `.cargo/config.toml`; `build-std` rebuilds `core`/`alloc` with it.
2. **The thunks.** Port `__x86_return_thunk`, `srso_return_thunk`,
   `srso_untrain_ret`, `retbleed_return_thunk`/`retbleed_untrain_ret` and the
   indirect thunks from `arch/x86/lib/retpoline.S` 1:1, including their
   alignment, which is part of the mitigation. Which thunk is live is chosen
   per CPU at boot, as Linux does with alternatives — here most likely an
   indirection that is itself not a `ret`.
3. **Hand-written assembly.** 7 `ret`/`iretq` in `kernel/src` today
   (`npk_clear_bhb_loop`, `npk_int_stubs`, interrupt returns). Each needs a
   decision: through the thunk, or provably outside the window (Linux keeps
   some bare `ret` with `ANNOTATE_UNRET_SAFE`).
4. **forge-generated code — the hard part.** WASM modules run as x86 code
   that forge emits into the kernel's address space. Its `ret`
   (`forge/core/src/x64.rs:736`), `jmp reg` (406), `jmp [mem]` (455) and
   `call reg` (791) must go through the same thunks, or a guest-trained
   predictor reaches the host through module code. The emitters are
   central, so this is a bounded change, but every module must be
   recompiled and the thunk addresses become part of forge's ABI.
5. **VM exit path.** Replace the IBPB with the untrain sequence Linux uses
   on this CPU, then drop `IBPB_ON_EXIT` for family 0x17 in
   `spec_mitigations::decide`. The full RSB fill after exit stays as Linux
   keeps it for retpoline mode (`RSB_VMEXIT`).

## Cost and risk

- Every `ret` in the kernel gets more expensive on every CPU where the thunk
  is live — Linux measures a few percent on syscall-heavy loads. On the NUC
  (eIBRS) and on AutoIBRS parts no thunk is needed; the build has to keep
  the plain `ret` reachable there (Linux patches the thunk back to `ret` at
  boot; we need an equivalent, or the cost lands on every CPU).
- Toolchain: the two `-Z` flags are unstable. A nightly that changes their
  behaviour silently changes the mitigation; `objdump` checks on the built
  kernel (no bare `ret` outside the allowed list) belong in the build.
- A mistake here does not crash; it silently reopens a leak. Every step needs
  a check that can fail: count bare `ret`/indirect branches in the image and
  in forge output.

## Payoff

Only on CPUs without eIBRS/AutoIBRS that are retbleed/SRSO-affected: Zen 1,
Zen 2, Zen 3 (SRSO), older Intel. About 0.6–0.9 µs per VM exit on the
IdeaPad, i.e. roughly half its exit cost, and the same share of every
cross-vCPU shootdown.

## Order if we do it

1. Build flags + thunks + boot-time selection, IBPB still on; prove with
   objdump that no bare `ret`/indirect branch is left outside the list.
2. forge: emit through the thunks; recompile all modules; same check on
   forge output.
3. Exit path: untrain sequence, IBPB off on 0x17; vmbench on the IdeaPad.
