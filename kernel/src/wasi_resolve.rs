// Path resolution for the wasi grant — the whole security boundary, in
// one file with no dependencies beyond String/Vec/format so the kernel
// and a host test can both use THESE bytes. A copy in a test crate would
// prove nothing about what actually runs.
//
// Included by `kernel/src/wasi.rs`; see `tools/wasi-path-test/` for the
// host side.

#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    /// The path would leave the granted subtree.
    Escape,
    /// NUL byte in a component.
    Invalid,
}

/// Resolve `rel` against `base`, refusing anything that leaves `root`.
///
/// `base` is where the directory fd points, `root` is the grant it
/// descends from, both npkFS paths without leading or trailing slashes.
/// A leading `/` in `rel` yields an empty first component and is
/// skipped: preview1 paths are always relative to a directory fd, so an
/// absolute-looking path lands inside the grant rather than beside it.
pub fn resolve_under(base: &str, root: &str, rel: &str) -> Result<String, Reject> {
    let root_depth = root.split('/').filter(|s| !s.is_empty()).count();
    let mut parts: Vec<String> =
        base.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();

    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                // The floor is the grant, not the filesystem root.
                if parts.len() <= root_depth { return Err(Reject::Escape); }
                parts.pop();
            }
            _ => {
                if c.contains('\0') { return Err(Reject::Invalid); }
                parts.push(c.to_string());
            }
        }
    }

    let out = parts.join("/");
    // Belt and braces. The walk above cannot produce an escape; if a
    // later edit makes it possible, the grant still holds. The `/` in
    // the prefix matters: without it "sys/pythonista" passes as being
    // under "sys/python".
    if out != root && !out.starts_with(&format!("{}/", root)) {
        return Err(Reject::Escape);
    }
    Ok(out)
}
