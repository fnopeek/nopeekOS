# `tools/wasm/testdisk/src/lib.rs` @ 5e0102684

## L1-10 · `#![no_std]`

```
//! testdisk — npkFS v2 benchmark + roundtrip validation.
//!
//! Run as `run testdisk`. For each size bucket (256 B, 4 KB, 64 KB,
//! 1 MB) we time WRITE then READ over `count` ops, report IOPS +
//! throughput, then delete to clean up. A small roundtrip check at
//! the end byte-compares one read per bucket so a silent corruption
//! shows up immediately.
//!
//! All paths under `.testdisk/`. Re-runs are safe (each store path
//! is delete-first).
```

## L31-40 · `const HEAP_SIZE: usize = 256 * 1024 * 1024;`

```
// ── 256 MB bump heap ──────────────────────────────────────────────────
//
// Sized for the 100 MB bucket: 100 MB write_buf + 100 MB read_buf + ~50
// MB slack for transient kernel-side allocations (storage::put alloc'd
// AES-GCM ciphertext + tree-rebuild + commit-journal scratch).
//
// 256 MB as bss adds nothing to the binary on disk (zero-init), but
// the wasmi runtime has to back it with real pages on instantiate, so
// startup gets slower — measured ~50 ms extra on N100. Acceptable for
// a one-shot bench.
```

## L65 · `fn print_dec(n: u64) { host::print_dec(n); }`

```
// ── Output formatting (no f64 in WASM no_std — keep integer-only) ─────
```

## L70 · `let mut tmp = [0u8; 24];`

```
// Right-align decimal `n` in `width` columns. Used for table rows.
```

## L89 · `fn fmt_size(bytes: usize) -> String {`

```
// ── Size formatting ──────────────────────────────────────────────────
```

## L101 · `const PLAN: &[(usize, u32, &str)] = &[`

```
// ── Benchmark plan ────────────────────────────────────────────────────
```

## L103-110 · `const PLAN: &[(usize, u32, &str)] = &[`

```
/// (size_bytes, count_per_phase, label_prefix) — kept small so a slow
/// WASM interpreter doesn't spend forever on the unmeasured loop overhead.
/// We're measuring kernel FS perf, not Rust→Wasm code-gen.
///
/// `xhuge` (16 MB × 1) probes the data-bandwidth ceiling: at this size
/// the indirect-extent chain has thousands of entries, and AEAD +
/// BLAKE3 are running over real bytes — what we measure here is the
/// NVMe queue-depth-1 bottleneck, not the FS-layer commit overhead.
```

## L122 · `struct PhaseStats {`

```
// ── Per-phase timer ───────────────────────────────────────────────────
```

## L143-144 · `fn kb_per_s(total_bytes: u64, us: u64) -> u64 {`

```
/// KB/s = bytes × 1000 / µs (decimal KB, close enough). KB rather
/// than MB so small-write rows don't round to zero from integer math.
```

## L150 · `fn run_phase(`

```
// ── Phase runner ──────────────────────────────────────────────────────
```

## L164 · `for i in 0..count {`

```
// Pre-clean any leftover from prior runs (cheap on a fresh fs).
```

## L170 · `let t0 = host::tsc_now();`

```
// ── WRITE ────────────────────────────────────────────────────────
```

## L173-175 · `let counter = i as u64;`

```
// Tweak the first 8 bytes per iteration so each blob has a
// distinct hash — without that, the storage layer's content
// dedup makes 2..N writes free and the throughput number lies.
```

## L187 · `let t2 = host::tsc_now();`

```
// ── READ ─────────────────────────────────────────────────────────
```

## L199 · `let counter = 0u64;`

```
// ── Roundtrip check on one entry per phase (catches silent corrupt) ─
```

## L213 · `for i in 0..count {`

```
// Cleanup
```

## L247 · `#[unsafe(no_mangle)]`

```
// ── Entry ─────────────────────────────────────────────────────────────
```

## L251-253 · `host::log("[testdisk] _start entry");`

```
// Direct-to-serial so the timestamp shows up the instant WASM
// starts executing. Compare with the kernel's "Running ..." log
// — gap between them = wasmi instantiate cost.
```

## L270-272 · `let max_size = 100 * 1024 * 1024 + 32;`

```
// 100 MB buffers (sized for the 100M bucket). Rust's `vec![0; N]`
// memsets which on N100 = ~30 ms × 2 buffers; wasmi's
// `memory.fill` lowers to host memset so it's near-DRAM-bandwidth.
```

## L291 · `let total_bytes: u64 = all.iter().map(|s| s.size as u64 * s.count as u64).sum();`

```
// Aggregate
```

## L316-318 · `host::print("\n  Crypto:    BLAKE3 ");`

```
// Ceiling probes — what the HW + crypto can do in isolation, vs
// the FS-stack throughput above. First sys_info(30..) call
// triggers the bench; the kernel caches the result until reboot.
```

## L333-334 · `host::print("\n  Self-check: ");`

```
// ── Integrity self-check (SAME run — a corrupt FS only shows up here;
// after a reboot the mount fails and we never get to run again) ────────
```

