# `forge/harness/src/wasi_core.rs` @ 5e0102684

## L1-7 · `#![allow(dead_code)]`

```
//! WASI preview1, lifted out of `../tools/pywasi` so BOTH engines can use the
//! SAME implementation. Comparing two compilers through two different host
//! layers would measure the host layers.
//!
//! Every function here works on `(guest memory, context)` and nothing else —
//! that was already true in pywasi, which is why the bodies transfer
//! unchanged. What differs per engine is only how those two are obtained.
```

## L10-20 · `use std::collections::BTreeMap;`

```
// `wasi_snapshot_preview1` on wasmi — host-side prototype of the shim
// nopeekOS would need in `kernel/src/wasm.rs`.
//
// Everything that touches the outside world goes through `Fs` at the
// bottom of this file. On the host that is `std::fs`; in the kernel it
// becomes npkFS, and nothing above it changes.
//
// Capability shape, kept deliberately: the guest can only reach paths
// below a preopened directory it was handed. There is no absolute-path
// escape hatch and no `..` climb above the root — a WASI program does
// not get the machine, it gets the subtree it was granted.
```

## L28-29 · `thread_local! {`

```
// Where a finishing run reports how long it took. A wasi program leaves
// through `proc_exit`, so the time has to be taken on the way out.
```

## L35 · `pub fn arm_report(fd: i32, start: std::time::Instant) {`

```
/// Arm the report before entering the module.
```

## L40-42 · `pub fn report(ctx: &WasiCtx, code: i32) {`

```
/// Send the run's time, its exit status and everything it wrote to stdout back
/// to the parent. Both engines leave through here, so both are measured the
/// same way and their output can be compared byte for byte.
```

## L54 · `unsafe {`

```
// SAFETY: `fd` is a pipe this process opened; `buf` is a live allocation.
```

## L60 · `pub const SUCCESS: i32 = 0;`

```
// ── errno ─────────────────────────────────────────────────────────────
```

## L74 · `const FT_DIR: u8 = 3;`

```
// ── filetype ──────────────────────────────────────────────────────────
```

## L79 · `const O_CREAT: i32 = 1;`

```
// ── oflags / fdflags / rights ─────────────────────────────────────────
```

## L92 · `Dir { path: PathBuf, guest: String, preopen: bool },`

```
/// `preopen` is set for the fds announced through `fd_prestat_get`.
```

## L99 · `root: PathBuf,`

```
/// Host directory the guest sees as its root.
```

## L103-104 · `pub stdout: Vec<u8>,`

```
/// Bytes written to fd 1 and 2, so a measurement run can stay quiet
/// and still prove the program produced the right output.
```

## L107-108 · `pub calls: BTreeMap<&'static str, u64>,`

```
/// Counts every host call, per function — this is the number that
/// tells us which preview1 calls the kernel shim must be fast at.
```

## L142-144 · `fn resolve(&self, dir_fd: i32, rel: &str) -> Result<PathBuf, i32> {`

```
/// Resolve a guest-relative path under `dir_fd`, refusing anything
/// that would leave the granted subtree. This is the whole security
/// boundary of the shim — it has to be the boring, obvious code.
```

## L159-160 · `Component::RootDir | Component::Prefix(_) => return Err(ENOTCAPABLE),`

```
// An absolute path or a prefix would step outside the
// grant by construction.
```

## L182 · `fn w32(m: &mut [u8], at: i32, v: u32) -> Result<(), i32> {`

```
// ── little-endian pokes into guest memory ─────────────────────────────
```

## L213 · `b[24..32].copy_from_slice(&1u64.to_le_bytes());          // nlink`

```
// nlink
```

## L225-230 · `#[allow(unused_variables, unused_mut)]`

```
// ── the 42 imports ────────────────────────────────────────────────────
//
// python.wasm imports exactly these, all from one namespace. Sixteen of
// them are stubs that return ENOSYS and CPython copes — sockets,
// symlinks, hard links. The ones that carry the startup are path_open,
// fd_read, fd_seek, fd_readdir and fd_filestat_get.
```

## L236-240 · `report(ctx, code);`

```
// A wasi program finishes by trapping out of here, clean run included.
// The interpreter can turn that into an `Err` and unwind; generated code
// has no trap path yet, so the run reports its own time on the way out and
// leaves. Both engines take this same exit, which is what keeps the two
// measurements comparable.
```

## L244 · `unsafe { libc::_exit(code) }`

```
// SAFETY: ends this process without unwinding, after the buffers are out.
```

## L308-310 · `let mut x: u64 = 0x9E3779B97F4A7C15;`

```
// Measurement harness: a counter, not entropy. The kernel shim
// must wire this to csprng — CPython seeds its hash randomisation
// from here, and a predictable seed is a real weakness.
```

## L332 · `b[8..16].copy_from_slice(&u64::MAX.to_le_bytes());   // rights_base`

```
// rights_base
```

## L333 · `b[16..24].copy_from_slice(&u64::MAX.to_le_bytes());  // rights_inheriting`

```
// rights_inheriting
```

## L378-379 · `let name_len = match ctx.fds.get(&fd) {`

```
// Only preopened dirs answer. The libc walks fds upward until it
// gets EBADF — that is how it learns where the grant ends.
```

## L385 · `b[0] = 0; // dir`

```
// dir
```

## L438-439 · `let mut total = 0u32;`

```
// Read into a staging buffer, then scatter — reading straight into
// guest memory would need two mutable borrows of `mem` at once.
```

## L482 · `let save = match f.stream_position() { Ok(p) => p, Err(e) => return errno_of(&e) };`

```
// Save and restore: pread must not move the cursor.
```

## L587-588 · `let mut names: Vec<(String, u8)> = vec![(".".into(), FT_DIR), ("..".into(), FT_DIR)];`

```
// `.` and `..` come first so the cookie space matches what a
// POSIX reader expects to walk.
```

## L603 · `rec.extend_from_slice(&((i as u64) + 1).to_le_bytes());   // d_next`

```
// d_next
```

## L604 · `rec.extend_from_slice(&((i as u64) + 1).to_le_bytes());   // d_ino`

```
// d_ino
```

## L609-610 · `let take = rec.len().min(cap - written);`

```
// A truncated final record is not an error: the reader sees
// bufused == buf_len and comes back with a bigger buffer.
```

## L626-627 · `match w32(mem, nev, 0) { Ok(()) => SUCCESS, Err(e) => e }`

```
// Nothing is ever ready. CPython only polls here for stdin and
// for its signal machinery, neither of which exists for us.
```

## L648-649 · `if let Ok(md) = std::fs::metadata(&full) {`

```
// A plain open of a directory still has to succeed — CPython
// stats through opened dir handles during import.
```

