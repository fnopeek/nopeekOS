# `tools/wasi-path-test/src/main.rs` @ 5e0102684

## L1-6 · `include!("../../../kernel/src/wasi_resolve.rs");`

```
//! Host test for the wasi grant boundary.
//!
//! `include!`s the SAME file the kernel compiles, so this exercises the
//! bytes that ship — not a re-implementation that could drift from them.
//!
//!   cargo run --manifest-path tools/wasi-path-test/Cargo.toml
```

## L26 · `ok(R, R, "lib/python313.zip", "sys/python/lib/python313.zip");`

```
// ── ordinary descent ──────────────────────────────────────────────
```

## L35 · `escapes(R, R, "..");`

```
// ── the boundary ──────────────────────────────────────────────────
```

## L42 · `escapes("home/florian", "home/florian", "../../sys/python");`

```
// Two grants must not become a path into one another.
```

## L45-46 · `ok(R, R, "/etc/passwd", "sys/python/etc/passwd");`

```
// An absolute-looking path is relative to the fd, so it stays inside
// the grant instead of reaching a real root.
```

## L50 · `escapes("sys/pythonista", R, "x");`

```
// Prefix confusion: "sys/pythonista" is NOT under "sys/python".
```

## L54 · `match resolve_under(R, R, "a\0b") {`

```
// NUL in a component.
```

## L60-63 · `let alphabet = ["a", "..", ".", "", "b", "/"];`

```
// ── exhaustive walk ───────────────────────────────────────────────
// Every sequence of up to 6 components from a small alphabet, from
// several starting depths. The invariant is the only thing that
// matters: whatever comes back is inside the grant, or nothing does.
```

