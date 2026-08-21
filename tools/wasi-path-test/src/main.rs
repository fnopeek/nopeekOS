//! Host test for the wasi grant boundary.
//!
//! `include!`s the SAME file the kernel compiles, so this exercises the
//! bytes that ship — not a re-implementation that could drift from them.
//!
//!   cargo run --manifest-path tools/wasi-path-test/Cargo.toml

include!("../../../kernel/src/wasi_resolve.rs");

fn ok(base: &str, root: &str, rel: &str, want: &str) {
    match resolve_under(base, root, rel) {
        Ok(got) if got == want => {}
        other => panic!("resolve({base:?}, {root:?}, {rel:?}) = {other:?}, want Ok({want:?})"),
    }
}
fn escapes(base: &str, root: &str, rel: &str) {
    match resolve_under(base, root, rel) {
        Err(Reject::Escape) => {}
        other => panic!("resolve({base:?}, {root:?}, {rel:?}) = {other:?}, want Escape"),
    }
}

fn main() {
    const R: &str = "sys/python";

    // ── ordinary descent ──────────────────────────────────────────────
    ok(R, R, "lib/python313.zip", "sys/python/lib/python313.zip");
    ok(R, R, "", "sys/python");
    ok(R, R, ".", "sys/python");
    ok(R, R, "a/./b", "sys/python/a/b");
    ok(R, R, "a/b/../c", "sys/python/a/c");
    ok("sys/python/lib", R, "..", "sys/python");
    ok("sys/python/lib/x", R, "../..", "sys/python");

    // ── the boundary ──────────────────────────────────────────────────
    escapes(R, R, "..");
    escapes(R, R, "../etc");
    escapes(R, R, "../../..");
    escapes("sys/python/lib", R, "../..");
    escapes("sys/python/lib", R, "a/../../../x");
    escapes(R, R, "a/../..");
    // Two grants must not become a path into one another.
    escapes("home/florian", "home/florian", "../../sys/python");

    // An absolute-looking path is relative to the fd, so it stays inside
    // the grant instead of reaching a real root.
    ok(R, R, "/etc/passwd", "sys/python/etc/passwd");
    ok(R, R, "//a//b", "sys/python/a/b");

    // Prefix confusion: "sys/pythonista" is NOT under "sys/python".
    escapes("sys/pythonista", R, "x");
    escapes("sys/python2", R, "");

    // NUL in a component.
    match resolve_under(R, R, "a\0b") {
        Err(Reject::Invalid) => {}
        other => panic!("NUL component = {other:?}, want Invalid"),
    }

    // ── exhaustive walk ───────────────────────────────────────────────
    // Every sequence of up to 6 components from a small alphabet, from
    // several starting depths. The invariant is the only thing that
    // matters: whatever comes back is inside the grant, or nothing does.
    let alphabet = ["a", "..", ".", "", "b", "/"];
    let bases = ["sys/python", "sys/python/lib", "sys/python/lib/deep"];
    let mut checked = 0u64;
    let mut escaped = 0u64;
    for base in bases {
        for n in 0..=6usize {
            let mut idx = vec![0usize; n];
            loop {
                let rel = idx.iter().map(|&i| alphabet[i]).collect::<Vec<_>>().join("/");
                match resolve_under(base, R, &rel) {
                    Ok(p) => assert!(
                        p == R || p.starts_with(&format!("{R}/")),
                        "leaked out of the grant: base={base:?} rel={rel:?} -> {p:?}"
                    ),
                    Err(Reject::Escape) => escaped += 1,
                    Err(Reject::Invalid) => {}
                }
                checked += 1;
                if n == 0 { break; }
                let mut k = n;
                loop {
                    if k == 0 { break; }
                    k -= 1;
                    idx[k] += 1;
                    if idx[k] < alphabet.len() { break; }
                    idx[k] = 0;
                    if k == 0 { k = usize::MAX; break; }
                }
                if k == usize::MAX { break; }
            }
        }
    }
    println!("wasi grant boundary: {checked} paths checked, {escaped} refused, none leaked");
}
