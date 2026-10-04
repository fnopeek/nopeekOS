# `tools/wasm/sdk/widgets/src/heap.rs` @ 5e0102684

## L1-17 · `#[cfg(target_arch = "wasm32")]`

```
//! A heap that GROWS, and gives back.
//!
//! Moved here from beak 0.188.0, where it had been since the fonts made a
//! never-freeing bump allocator run out of memory. It is shared rather than
//! copied because the two interesting numbers in it — the doubling step and
//! its cap — were each paid for once, and a second copy would have to pay
//! again.
//!
//! Use it when the app allocates and frees repeatedly: fonts, layout trees,
//! decoded frames. An app that allocates once and lives off it (`top`,
//! `volume`) is better served by a bump heap and should not turn the
//! `heap` feature on at all.
//!
//! `​``ignore
//! #[global_allocator]
//! static ALLOC: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();
//! `​``
```

## L27 · `unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32) }`

```
// SAFETY: pointer + length of a live `&str`; the host only reads it.
```

## L33-41 · `#[derive(Debug)]`

```
/// Ported from talc's own `WasmGrowAndExtend` (talc 5.0.4, `src/wasm.rs`)
/// with exactly one change: the growth STEP.
///
/// Talc grows by just enough for the allocation that failed. That is right
/// where `memory.grow` is cheap. Under wasmi it is not — linear memory is one
/// contiguous buffer, so every grow copies all of it, and growing a page at a
/// time up to a 60 MB working set would copy tens of gigabytes. Doubling makes
/// the number of grows logarithmic and the total copying linear in the final
/// size, at the cost of holding up to twice the peak.
```

## L44-50 · `end: usize,`

```
/// End of the arena talc last received, so a contiguous grow can EXTEND it
/// instead of starting a second heap. Zero means "nothing handed over yet".
///
/// An address and not a `NonNull`, which is what talc's own source stores:
/// `NonNull` is not `Send`, and this allocator sits behind a mutex on
/// purpose (see the note on `Allocator`) rather than behind talc's
/// single-threaded cell.
```

## L62-63 · `const GROW_CAP_PAGES: usize = 1024;`

```
/// Largest growth step: 64 MB. See `acquire` — doubling without a cap turns,
/// past a few hundred MB, into a demand the device cannot meet.
```

## L66-69 · `#[cfg(target_arch = "wasm32")]`

```
// SAFETY: `acquire` hands talc only memory that `memory.grow` just returned —
// freshly mapped pages past the previous end of linear memory, which nothing
// else can reach. It allocates nothing itself. That is talc's own
// `WasmGrowAndExtend` contract, kept.
```

## L76-77 · `let need = layout.size() + layout.align() + 4 * WASM_PAGE;`

```
// Over-estimate deliberately: talc warns that UNDER-sizing here loops
// forever, handing over heaps that can never fit the allocation.
```

## L81-85 · `let step = have_pages.max(1).min(GROW_CAP_PAGES);`

```
// **Double, but not forever.** Up to 64 MB doubling is right: few
// requests, little waste. Past that it becomes a demand the device
// CANNOT meet — with a 512 MB heap beak asked for another 512 MB, the
// kernel could not map it, and the crash looked like "out of memory"
// although four pages would have done.
```

## L89-91 · `let mut delta = delta;`

```
// Ask smaller in turn instead of giving up at the first no. The device
// says no to half a gigabyte and yes to four pages — and the four
// pages are what the caller actually needs.
```

## L97-99 · `log("[npk] Halde erschoepft — memory.grow hat nein gesagt");`

```
// Now the machine really is full — and that belongs in the log
// with NUMBERS, not as a bare panic line. Without allocating:
// we are inside the allocator.
```

## L114-115 · `let new_end = unsafe {`

```
// SAFETY: contiguous with the arena we handed over last time, and
// `old_end` came from talc itself, so it is non-null.
```

## L125 · `talc.source.end = unsafe { talc.claim(base, size) }.map_or(0, |e| e.as_ptr() as usize);`

```
// SAFETY: fresh pages, owned by nothing else.
```

## L131-136 · `#[cfg(target_arch = "wasm32")]`

```
/// `TalcLock` (mutex-guarded), NOT talc's `WasmArenaTalc`/`TalcSyncCell`. The
/// cell variants are only sound on single-threaded WebAssembly and enforce
/// that with a target check, not the type system — so the day an app gets
/// workers, or wasmi turns on the threads proposal, they would go quietly
/// unsound. The uncontended spin lock costs a few instructions; that is the
/// cheaper mistake.
```

## L145-146 · `static mut NUMBUF: [u8; 12] = [0; 12];`

```
// u32 → decimal &str in a static buffer (no alloc — safe in the panic handler
// even when the panic IS an allocation failure).
```

## L149-151 · `pub fn u32_str(mut n: u32) -> &'static str {`

```
/// `u32` to decimal without allocating, into a static buffer. Public
/// because a panic handler needs exactly this and must not allocate — the
/// panic may BE an allocation failure.
```

## L157 · `unsafe { *b.add(i) = b'0' };`

```
// SAFETY: `i` is in range of the 12-byte buffer.
```

## L162 · `unsafe { *b.add(i) = b'0' + (n % 10) as u8 };`

```
// SAFETY: as above.
```

## L166-167 · `unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(b.add(i), 12 - i)) }`

```
// SAFETY: the bytes written are ASCII digits, and the slice is inside
// the static buffer, which lives for the whole program.
```

