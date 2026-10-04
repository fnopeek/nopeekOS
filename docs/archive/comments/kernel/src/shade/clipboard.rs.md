# `kernel/src/shade/clipboard.rs` @ 5e0102684

## L1-16 · `use alloc::string::String;`

```
//! Cross-app clipboard — a single kernel-owned selection buffer.
//!
//! Written/read by the *focused* widget app only: compositor-internal for
//! TextArea Ctrl+C/X/V (it owns the live buffer), or via the
//! `npk_clipboard_*` host fns for app-driven copy (loft filenames, loop
//! scrollback). Eager, unlike X11/Wayland's lazy ownership model — the
//! data survives the source app closing.
//!
//! Two flavors, picked by data kind (Architecture principle #3:
//! content-addressed, not value-copied, for large data):
//!   - `Text`:   small inline UTF-8 bytes. ~all of copy/paste.
//!   - `Object`: a content-address handle (npkFS BLAKE3 hash + name + size)
//!               for files/large blobs — copies the 32-byte reference, never
//!               the bytes, so it is unbounded by construction. Reserved;
//!               loft's file-copy wires it later (no ABI break — it is just
//!               another variant).
```

## L23-26 · `const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;`

```
/// Hard cap on inline `Text` payloads. NOT a UX limit — you never select
/// 4 MiB of text by hand — only an anti-heap-exhaustion guard so a
/// malicious app can't balloon kernel memory. Large data uses `Object`
/// (a reference), never inline bytes.
```

## L29 · `#[allow(dead_code)] // Object lands with loft file-copy.`

```
// `Object` lands with loft file-copy.
```

## L37-38 · `static GENERATION: AtomicU64 = AtomicU64::new(0);`

```
/// Bumped on every write so a future microvm/agent bridge (and apps) can
/// cheaply detect "did the clipboard change?" without diffing bytes.
```

## L41-42 · `pub fn set_text(bytes: &[u8]) {`

```
/// Replace the clipboard with UTF-8 text. Truncates at `MAX_TEXT_BYTES`
/// on a char boundary. Empty input clears the clipboard.
```

## L45 · `let mut end = MAX_TEXT_BYTES;`

```
// Back up to a UTF-8 boundary so `Text` never holds a split codepoint.
```

## L60 · `pub fn get_text() -> Option<Vec<u8>> {`

```
/// Current clipboard text, or `None` if empty / holding a non-text kind.
```

## L68-69 · `pub fn text_len() -> usize {`

```
/// Byte length of the current text payload (0 if empty / non-text). Lets
/// an app size its receive buffer before calling `npk_clipboard_get`.
```

## L77 · `#[allow(dead_code)]`

```
/// Monotonic write counter (reserved for the microvm clipboard bridge).
```

