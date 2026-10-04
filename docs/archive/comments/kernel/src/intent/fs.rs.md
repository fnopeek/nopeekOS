# `kernel/src/intent/fs.rs` @ 5e0102684

## L1 · `use crate::{kprint, kprintln, capability};`

```
//! Filesystem intents: store, fetch, cat, grep, head, wc, hexdump, delete, mkdir, rmdir, list, fsinfo, disk
```

## L7-8 · `pub(super) fn fetch_object(name: &str) -> Option<alloc::vec::Vec<u8>> {`

```
/// Helper: fetch an npkFS object and return its data.
/// Name is resolved relative to cwd.
```

## L17 · `pub(super) fn parse_redirect(args: &str) -> (&str, Option<&str>) {`

```
/// Parse "args > target" redirect syntax. Returns (args, Option<store_name>).
```

## L45 · `if let Some(idx) = path.rfind('/') {`

```
// Auto-create parent directories
```

## L222 · `let (name, limit) = if let Some((n, l)) = args.rsplit_once(' ') {`

```
// Optional limit: hexdump name 128
```

## L249 · `for j in chunk.len()..16 {`

```
// Padding for short lines
```

## L345 · `for e in &entries {`

```
// Directories first, then files (entries arrive sorted by name).
```

## L353 · `if e.name.starts_with(".npk-") { continue; }`

```
// Hide kernel-internal entries from user listings.
```

## L429 · `if let Some(d) = crate::nvme::msix_debug() {`

```
// Live MSI-X read-back: did our table programming stick?
```

## L439 · `if let Some((total_blocks, free_blocks, object_count, _gen)) = crate::npkfs::stats() {`

```
// ── Filesystem (npkFS) + GC status ────────────────────────
```

## L479-481 · `pub fn crypto_bench() -> (u64, u64, u64) {`

```
/// Micro-bench BLAKE3 + AES-256-GCM on a 1 MB buffer to confirm the
/// HW backends (AVX2 / AES-NI) fire at runtime. Returns
/// `(blake3_mbs, aes_enc_mbs, aes_dec_mbs)`.
```

## L530-531 · `let ct_ref = crate::crypto::aead_encrypt_aes(&key, &nonce, &buf);`

```
// ── Custom AES-GCM (aead_hw.rs) ────────────────────────────────
// Validate first: encrypt with both, compare ciphertext+tag.
```

## L539 · `let t0 = rdtsc();`

```
// HW encrypt bench
```

## L549 · `let template_hw = crate::crypto::aead_encrypt_aes_hw(&key, &nonce, &buf);`

```
// HW decrypt-in-place bench
```

## L581-584 · `fn print_cpu_features() {`

```
/// Print HW feature bits (raw CPUID) alongside the cpufeatures crate's
/// runtime view of the same features. A mismatch — HW reports
/// pclmulqdq but the crate sees `false` — explains why aes-gcm /
/// blake3 might pick a soft path in spite of `target-feature = +pclmulqdq`.
```

## L588 · `let cpuid1 = __cpuid(1);`

```
// CPUID(1) ECX bits we care about for crypto + SIMD.
```

## L597-601 · `let cpuid7 = __cpuid(7);`

```
// CPUID(7,0) — extended features. EBX bit 5 = AVX2.
// ECX bit 10 = VPCLMULQDQ (VEX/EVEX-encoded carry-less multiply,
// 256/512-bit parallel — Ice Lake / Zen 3+. polyval 0.7+ requires
// this for its HW path; N100 (Atom-class) likely does NOT have it
// even though plain PCLMULQDQ works.)
```

## L606-607 · `cpufeatures::new!(crate_aes,         "aes");`

```
// What the cpufeatures crate's runtime detection actually says —
// same primitives aes-gcm / blake3 see.
```

## L664 · `let mut prev = [0xFFu8; 16]; // impossible first value`

```
// impossible first value
```

