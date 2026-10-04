# `kernel/src/wasi/calls.rs` @ 5e0102684

## L1-12 · `#![allow(clippy::too_many_arguments)]`

```
//! wasi preview1, motorneutral.
//!
//! Jede Funktion hier arbeitet auf `(&mut [u8], &mut HS)` und sonst nichts —
//! genau das Paar, das das `state!`-Makro im Interpreterpfad schon lieferte.
//! Deshalb sind die Rumpfe unveraendert herueber gekommen; was sich je Motor
//! unterscheidet, ist nur, WIE die beiden beschafft werden.
//!
//! `proc_exit` steht bewusst NICHT hier: es darf nicht zurueckkehren, und wie
//! man ein Modul verlaesst, ist das eine, was die Motoren wirklich
//! unterscheidet. Siehe `wasi.rs`.
//!
//! DIESE DATEI IST ERZEUGT.
```

## L53-54 · `match w64(mem, out, 10_000_000) { Ok(()) => SUCCESS, Err(e) => e }`

```
// The tick is 100 Hz, and saying so beats claiming nanoseconds
// we cannot deliver.
```

## L59 · `let now = if id == 0 {`

```
// 0 = realtime, everything else treated as monotonic.
```

## L70-72 · `let l = match usize::try_from(len) { Ok(v) => v, Err(_) => return EFAULT };`

```
// Straight from the kernel CSPRNG. CPython seeds its hash
// randomisation here, so a predictable stream is a real
// weakness, not a placeholder detail.
```

## L88-89 · `let is_std = matches!(`

```
// stdout/stderr leave through the same door as npk_print, so a
// wasi program lands in the terminal the user is looking at.
```

## L126-127 · `Some(Handle::Stdin) => Vec::new(),`

```
// Nothing types into a wasi program yet: it gets EOF, not
// a hang. A REPL will need a real stdin here.
```

## L155-156 · `let chunk: Vec<u8> = match w.fds.get(&fd) {`

```
// pread must not move the cursor — with the bytes already in
// hand that is just not touching `pos`.
```

## L227-229 · `Some(Handle::File { path, data, dirty: true, .. }) => {`

```
// Whole-file store: the write-back happens here, once, not
// on every fd_write. A close that fails must say so — a
// silently dropped buffer is a lost file.
```

## L290-291 · `let name_len = match w.fds.get(&fd) {`

```
// wasi-libc walks fds upward until one answers EBADF — that is
// how it learns where the grant ends. Only preopens answer.
```

## L297 · `b[0] = 0; // dir`

```
// dir
```

## L335 · `rec.extend_from_slice(&((i as u64) + 1).to_le_bytes()); // d_next`

```
// d_next
```

## L336 · `rec.extend_from_slice(&((i as u64) + 1).to_le_bytes()); // d_ino`

```
// d_ino
```

## L341-342 · `let take = rec.len().min(cap - written);`

```
// A truncated last record is not an error: the reader sees
// bufused == buf_len and comes back with a bigger buffer.
```

## L356-357 · `match w32(mem, nev, 0) { Ok(()) => SUCCESS, Err(e) => e }`

```
// Nothing is ever ready. CPython polls here for stdin and for
// its signal machinery; neither exists for us yet.
```

## L371-373 · `if !grant_writable { return EPERM; }`

```
// Two gates, both required: the grant this path came through
// has to allow writing, and the capability the run carries
// has to include WRITE. Either alone would be a hole.
```

## L381-383 · `let fd = w.insert(Handle::Dir {`

```
// A subdirectory inherits the grant it was reached
// through — that is what keeps two preopens from
// becoming a path into one another.
```

## L404-406 · `let fresh = existing.is_none() || oflags & O_TRUNC != 0;`

```
// A file created here does not exist in npkFS until close writes
// it back, so it starts dirty — otherwise `open(w); close()`
// would leave nothing behind.
```

## L466-467 · `if !wa || !wb || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }`

```
// Both ends must be writable: renaming out of a read-only grant
// is a delete, renaming into one is a write.
```

