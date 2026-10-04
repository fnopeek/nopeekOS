# `tools/wasm/testdisk/src/host.rs` @ 5e0102684

## L1 · `#![allow(dead_code)]`

```
//! Host function bindings for testdisk.wasm.
```

## L5-7 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L11-12 · `fn npk_log(ptr: i32, len: i32);`

```
/// Direct-to-kernel-serial — bypasses the per-app output buffer
/// `npk_print` uses, so the message appears in real time.
```

## L22-24 · `fn npk_sys_info(key: i32) -> i64;`

```
/// Generic system-info accessor.
/// Key 10 → TSC frequency in MHz.
/// Key 19 → raw TSC ticks (monotonic).
```

## L31-33 · `pub fn bench_blake3_mbs() -> u64    { unsafe { npk_sys_info(30) as u64 } }`

```
/// Bench probes — kernel-side, AVX2/AES-NI/raw-NVMe pathway.
/// Keys 30..34. First call triggers ~100 ms of measurement, results
/// are cached in the kernel until reboot.
```

## L40-44 · `pub fn fs_selfcheck() -> i64 { unsafe { npk_sys_info(40) } }`

```
/// Read-only FS integrity self-check (key 40). Runs the kernel's btree
/// refcount scan NOW (not cached) and returns the total problem count
/// (0 = clean, -1 = scan error). The detailed report is logged to serial
/// by the kernel. Called at the end of a run so corruption surfaces before
/// a reboot bricks the mount.
```

## L51 · `pub fn log(s: &str) {`

```
/// Live-to-serial logging (used for "where did the time go" diagnostics).
```

## L56-57 · `pub fn store(name: &str, data: &[u8]) -> bool {`

```
/// Strict create — fails if `name` already exists. Caller is expected
/// to delete first if overwrite is desired.
```

## L65 · `pub fn fetch(name: &str, buf: &mut [u8]) -> i32 {`

```
/// Returns bytes written into `buf` on success, -1 on error / not found.
```

## L73-74 · `pub fn fs_list(prefix: &str, buf: &mut [u8], recursive: bool) -> i32 {`

```
/// Returns bytes written into `buf`. 0 if the prefix is empty / has no
/// children, -1 on error.
```

## L83-84 · `pub fn fs_stat(name: &str, out: &mut [u8; 17]) -> i32 {`

```
/// 17 → wrote (size_u64 + is_dir_u8 + mtime_u64); 0 → not found;
/// -1 → error. Kernel ≥ v0.146 returns 17; older kernels returned 9.
```

## L95 · `pub fn print_dec(n: u64) {`

```
/// Decimal print to terminal (no `format!`, no allocation).
```

