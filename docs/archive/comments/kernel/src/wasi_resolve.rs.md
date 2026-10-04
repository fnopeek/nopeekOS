# `kernel/src/wasi_resolve.rs` @ 5e0102684

## L1-7 · `#[derive(Debug, PartialEq, Eq)]`

```
// Path resolution for the wasi grant — the whole security boundary, in
// one file with no dependencies beyond String/Vec/format so the kernel
// and a host test can both use THESE bytes. A copy in a test crate would
// prove nothing about what actually runs.
//
// Included by `kernel/src/wasi.rs`; see `tools/wasi-path-test/` for the
// host side.
```

## L11 · `Escape,`

```
/// The path would leave the granted subtree.
```

## L13 · `Invalid,`

```
/// NUL byte in a component.
```

## L17-23 · `pub fn resolve_under(base: &str, root: &str, rel: &str) -> Result<String, Reject> {`

```
/// Resolve `rel` against `base`, refusing anything that leaves `root`.
///
/// `base` is where the directory fd points, `root` is the grant it
/// descends from, both npkFS paths without leading or trailing slashes.
/// A leading `/` in `rel` yields an empty first component and is
/// skipped: preview1 paths are always relative to a directory fd, so an
/// absolute-looking path lands inside the grant rather than beside it.
```

## L33 · `if parts.len() <= root_depth { return Err(Reject::Escape); }`

```
// The floor is the grant, not the filesystem root.
```

## L45-48 · `if out != root && !out.starts_with(&format!("{}/", root)) {`

```
// Belt and braces. The walk above cannot produce an escape; if a
// later edit makes it possible, the grant still holds. The `/` in
// the prefix matters: without it "sys/pythonista" passes as being
// under "sys/python".
```

