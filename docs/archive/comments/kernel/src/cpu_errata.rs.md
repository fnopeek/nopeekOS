# `kernel/src/cpu_errata.rs` @ 5e0102684

## L1-5 · `const MSR_DE_CFG: u32 = 0xC001_1029;`

```
//! Per-core CPU mitigations the host must set itself — Linux `init_amd_zen2`.
//!
//! A microvm guest used to set these on the host core through pass-through
//! MSRs. Now that its MSR writes stay in the guest, the host does it, on
//! every core, before any guest can run there.
```

## L20 · `unsafe {`

```
// SAFETY: only called for MSRs Linux documents on Zen2 (see `apply`).
```

## L29 · `unsafe {`

```
// SAFETY: read-modify-write of a Zen2 chicken bit Linux sets the same way.
```

## L36 · `fn amd_family_model() -> Option<(u32, u32)> {`

```
/// (family, model) per CPUID leaf 1, AMD encoding.
```

## L39 · `if (b, d, c) != (0x6874_7541, 0x6974_6E65, 0x444D_4163) { return None; }`

```
// "AuthenticAMD"
```

## L42 · `if ecx1 & (1 << 31) != 0 { return None; }`

```
// Under a hypervisor these MSRs are its business (Linux skips too).
```

## L54 · `fn zenbleed_fixed(model: u32, rev: u32) -> bool {`

```
/// First microcode revision with the Zenbleed fix (`cpu_has_zenbleed_microcode`).
```

## L67 · `pub fn apply() {`

```
/// Run on every core (BSP and each AP) during bring-up.
```

## L71 · `wrmsr(MSR_ZEN2_SPECTRAL_CHICKEN, rdmsr(MSR_ZEN2_SPECTRAL_CHICKEN) | SPECTRAL_CHICKEN_BIT);`

```
// Retbleed: suppress non-branch predictions.
```

## L73-74 · `let rev = rdmsr(MSR_PATCH_LEVEL) as u32;`

```
// Zenbleed: without fixed microcode the chicken bit is the mitigation —
// otherwise vector registers leak across contexts, guest ↔ host included.
```

