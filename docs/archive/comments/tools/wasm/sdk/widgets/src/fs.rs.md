# `tools/wasm/sdk/widgets/src/fs.rs` @ 5e0102684

## L1-16 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
//! Decoder for the `npk_fs_list` output buffer.
//!
//! The kernel writes one record per entry, records joined by `\n`:
//!
//! `​``text
//! <name> \0 <size:u64 LE> \0 <is_dir:u8> \0 <mtime:u64 LE>
//! `​``
//!
//! The name is NUL-terminated, so the 19 bytes after it are a fixed-width
//! tail — the record is unambiguous when read **sequentially**. It is not
//! when the buffer is split on `\n` first: `size` and `mtime` are raw
//! little-endian integers and any of their bytes can be `0x0A`. A file of
//! 2600 bytes (`0x0A28`) or a mtime whose low byte happens to be 10 tears
//! its own record in half — the front half is dropped for a short tail,
//! and the back half decodes as a nameless directory carrying garbage.
//! Roughly one entry in sixty. Read records, never lines.
```

## L18 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// One decoded directory entry. Borrows the name out of the buffer.
```

## L24 · `pub mtime:  u64,`

```
/// UTC seconds since the Unix epoch; zero means unknown.
```

## L28 · `const TAIL: usize = 19;`

```
/// `size(8) + sep + is_dir(1) + sep + mtime(8)`.
```

## L31-32 · `pub fn list_entries(buf: &[u8]) -> ListIter<'_> {`

```
/// Decode an `npk_fs_list` buffer. Pass the first `n` bytes the host fn
/// reported written; a non-positive return has no entries to decode.
```

## L48-49 · `if self.buf.len() < tail_at + TAIL {`

```
// A record that cannot hold its own tail means the buffer is
// damaged; stop rather than resynchronise on a guess.
```

## L61-63 · `let Ok(name) = name else { continue };`

```
// A nameless entry is never legitimate, and non-UTF-8 cannot be
// shown or passed back as a path. Skip and keep going — the
// records after it are still intact.
```

## L117 · `#[test]`

```
/// The bug this module exists for: a `0x0A` byte inside `size`.
```

## L133 · `#[test]`

```
/// …and one inside `mtime`, which also cost the entry its timestamp.
```

## L136 · `for mtime in [1786110474u64, 1786055168, 10] {`

```
// 1786110474 has 0x0A as its low byte, 1786055168 as its second.
```

## L144 · `#[test]`

```
/// A name may legally contain a newline; only the NUL ends it.
```

