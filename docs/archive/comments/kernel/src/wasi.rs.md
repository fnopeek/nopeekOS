# `kernel/src/wasi.rs` @ 5e0102684

## L1-31 · `#![allow(dead_code)]`

```
//! `wasi_snapshot_preview1` — the second ABI.
//!
//! Everything else in this kernel talks `npk_*`: 123 functions that were
//! designed here, for a system with capabilities instead of permissions
//! and content addresses instead of a path tree. This module is the one
//! place that speaks somebody else's language, and it exists because the
//! programs worth borrowing — CPython, lua, sqlite — were all written
//! against POSIX and compiled through wasi-libc.
//!
//! It is deliberately NOT a Python feature. A guest that gets this
//! namespace gets a filesystem-shaped view of ONE npkFS subtree it was
//! handed, and nothing else. That framing is what makes the POSIX dent
//! in the architecture worth its price: every wasi binary lands the same
//! way, under the same grant, with the same ceiling.
//!
//! ## The shape of the grant
//!
//! `path_open` and friends resolve only under a preopened directory the
//! caller passed in. `resolve` refuses absolute escapes and any `..`
//! that would climb above the root. There is no way to name a path
//! outside the grant, so "which files can this program see" is answered
//! once, at spawn, by whoever built the `WasiCtx` — not by the program.
//!
//! ## Whole-file storage
//!
//! npkFS reads and writes whole objects; there is no seek at the storage
//! layer. So an opened file is fetched once into the fd entry and served
//! from there, and a written file is flushed on close. That is a real
//! memory cost — CPython holds an 8.5 MB stdlib zip open for its whole
//! run — and it is the honest shape for a content-addressed store: a
//! blob has a hash, and half a blob does not.
```

## L48 · `pub const SUCCESS: i32 = 0;`

```
// ── errno (preview1 numbering — do not renumber) ──────────────────────
```

## L64 · `const FT_CHR: u8 = 2;`

```
// ── filetype ──────────────────────────────────────────────────────────
```

## L69 · `const O_CREAT: i32 = 1;`

```
// ── oflags / fdflags / rights ─────────────────────────────────────────
```

## L77-80 · `pub enum Handle {`

```
/// An open descriptor.
///
/// `File` carries the bytes because npkFS has no seek — see the module
/// header. `dirty` decides whether closing has to write back.
```

## L93 · `path: String,`

```
/// npkFS path.
```

## L95 · `guest: String,`

```
/// What the guest calls it. Only meaningful for preopens.
```

## L98-101 · `root: String,`

```
/// The grant this handle descends from. `..` may not climb past
/// it, and every subdirectory opened through it inherits it —
/// which is what lets several preopens coexist without one
/// becoming a door into another.
```

## L112-114 · `exit_status: Option<i32>,`

```
/// Womit sich das Programm verabschiedet hat. Der Trap-Code sagt nur DASS
/// es sich beendet hat; der Status gehoert hierher, weil ihn beide Motoren
/// auf demselben Weg hinterlegen.
```

## L127-131 · `pub fn preopen(&mut self, npkfs_path: &str, guest: &str, writable: bool) {`

```
/// Hand the guest one npkFS directory under the name `guest`.
///
/// Call order matters only in that preopen fds must be contiguous
/// from 3 — wasi-libc discovers them by walking upward until one
/// answers EBADF, and a hole would hide everything after it.
```

## L151-158 · `fn resolve(&self, dir_fd: i32, rel: &str) -> Result<(String, String, bool), i32> {`

```
/// Resolve a guest path under `dir_fd` to an npkFS path.
///
/// This is the whole security boundary, so it stays boring: split
/// into components, refuse `..` at the floor, rebuild. A leading `/`
/// contributes an empty component and is skipped — preview1 paths
/// are always relative to the directory fd, so an absolute-looking
/// path still lands inside the grant rather than beside it.
/// Returns `(npkfs_path, grant_root, writable)`.
```

## L175-176 · `fn fs_errno(e: &fs::Error) -> i32 {`

```
/// `fs::Error` is npkFS's `PathError` under an alias — see
/// `storage/npkfs/fs.rs`.
```

## L189 · `fn mem_of(caller: &mut Caller<'_, crate::wasm::HostState>) -> Result<Memory, i32> {`

```
// ── guest memory pokes ────────────────────────────────────────────────
```

## L214-219 · `fn bytes_at(m: &[u8], ptr: i32, len: i32) -> Result<&[u8], i32> {`

```
/// Copy `len` bytes out of guest memory at `ptr`.
///
/// `try_from` on every offset, not `as usize`: a negative i32 from a
/// buggy or hostile guest becomes a huge usize under `as`, and that is
/// the shape of the overflow already logged against ~25 `npk_*`
/// functions. Not repeating it here.
```

## L248-253 · `macro_rules! wasi_of {`

```
// ── the 42 imports ────────────────────────────────────────────────────
//
// python.wasm imports exactly these. Ten answer ENOSYS on purpose —
// sockets, symlinks, hard links, the *_set_times family. CPython treats
// those failures as "this filesystem cannot do that", which is the
// truth: npkFS has no links and no per-file timestamps to set.
```

## L255-258 · `macro_rules! wasi_of {`

```
/// Grab guest memory and the host state together. Every function starts
/// here, and a run that was never granted a `WasiCtx` bounces on the
/// second step — the namespace is always linked, but it is inert unless
/// someone deliberately built a grant.
```

## L273 · `pub(crate) fn record_exit(st: &mut HS, code: i32) {`

```
/// Was `proc_exit` hinterlaesst — von beiden Motoren gleich geschrieben.
```

## L280-281 · `pub fn exit_status(st: &HS) -> Option<i32> {`

```
/// Und wieder heraus. `None` heisst: das Programm ist normal aus `_start`
/// zurueckgekehrt, ohne sich zu verabschieden.
```

## L289-293 · `linker.func_wrap(NS, "proc_exit",`

```
// ── process ───────────────────────────────────────────────────────
// Das EINZIGE, was die Motoren wirklich unterscheidet. Der Effekt ist
// gemeinsam — den Status hinterlegen —, das Verlassen nicht: der
// Interpreter macht daraus ein `Err` und rollt ab, erzeugter Code nimmt
// `forge_rt::host_trap`. Beides endet im selben Zustand.
```

## L305 · `linker.func_wrap(NS, "args_sizes_get", |mut c: Caller<'_, HS>, n: i32, sz: i32| -> i32 {`

```
// ── argv / environ ────────────────────────────────────────────────
```

## L327 · `linker.func_wrap(NS, "clock_res_get", |mut c: Caller<'_, HS>, _id: i32, out: i32| -> i32 {`

```
// ── clocks + randomness ───────────────────────────────────────────
```

## L365 · `fn gather(mem: &[u8], iovs: i32, iovs_len: i32) -> Result<Vec<u8>, i32> {`

```
// ── fd: the read/write path ───────────────────────────────────────────
```

## L367-368 · `fn gather(mem: &[u8], iovs: i32, iovs_len: i32) -> Result<Vec<u8>, i32> {`

```
/// Gather the `iovec` array into one buffer. Copying first keeps the
/// borrow of guest memory apart from the borrow of the fd table.
```

## L482 · `fn link_path(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {`

```
// ── path-based calls ──────────────────────────────────────────────────
```

## L525-530 · `fn link_stubs(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {`

```
// ── the ten that answer "no" ──────────────────────────────────────────
//
// Not laziness: npkFS has no links, no symlinks and no settable
// timestamps, and there are no sockets behind this ABI. ENOSYS is the
// true answer, and CPython handles it — `os.symlink` raises
// OSError, which is what it should do on a filesystem without symlinks.
```

## L592-594 · `pub fn ctx(args: Vec<String>, env: Vec<String>) -> Box<WasiCtx> {`

```
/// Build a context. Callers add their preopens with `preopen` — there
/// is no default grant, because "what can this program see" should be a
/// decision somebody wrote down, not a fallback.
```

