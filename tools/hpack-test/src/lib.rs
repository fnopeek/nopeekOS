//! Host-side oracle for the kernel's HPACK codec (RFC 7541).
//!
//! The kernel is a `no_std` binary, so `cargo test` cannot run inside it —
//! a host build hits a duplicate `_start` at link time. This crate pulls the
//! REAL kernel sources in via `#[path]` (std supplies `alloc`), so the tested
//! code and the shipped code are the same bytes and cannot drift.
//!
//! Run: `cargo test --manifest-path tools/hpack-test/Cargo.toml`
//!
//! `rfc_vectors.rs` is generated from RFC 7541 Appendix C — all 16 worked
//! examples, including the Huffman ones and the size-256 response sequence
//! that exercises dynamic-table eviction. `hostile.rs` asserts the property
//! that matters in a kernel: malformed input may be rejected, never panic.
extern crate alloc;

#[path = "../../../kernel/src/intent/http2/tables.rs"]
pub mod tables;

#[path = "../../../kernel/src/intent/http2/hpack.rs"]
pub mod hpack;

#[cfg(test)]
mod rfc_vectors;

#[cfg(test)]
mod hostile;

#[cfg(test)]
mod table_props {
    use super::tables::HUFFMAN;

    /// The decoder assumes the code is canonical. If a future edit to the
    /// generated table broke that, decoding would go subtly wrong rather than
    /// fail loudly — so assert the property directly.
    #[test]
    fn huffman_is_canonical_and_complete() {
        let mut by_len: Vec<Vec<(u32, usize)>> = vec![Vec::new(); 31];
        for (sym, &(code, len)) in HUFFMAN.iter().enumerate() {
            assert!((1..=30).contains(&len), "sym {sym} len {len}");
            assert!(code < (1u32 << len), "sym {sym} code wider than its length");
            by_len[len as usize].push((code, sym));
        }
        let mut kraft = 0f64;
        let mut prev: Option<(u32, u32, usize)> = None; // (first code, count, len)
        for len in 1..=30usize {
            let mut v = by_len[len].clone();
            if v.is_empty() { continue; }
            v.sort();
            kraft += v.len() as f64 * 2f64.powi(-(len as i32));
            for w in v.windows(2) {
                assert_eq!(w[1].0, w[0].0 + 1, "len {len}: codes not consecutive");
                assert!(w[1].1 > w[0].1, "len {len}: symbol order != code order");
            }
            if let Some((pf, pc, pl)) = prev {
                assert_eq!(v[0].0, (pf + pc) << (len - pl), "len {len}: not canonical");
            }
            prev = Some((v[0].0, v.len() as u32, len));
        }
        assert!((kraft - 1.0).abs() < 1e-12, "Kraft sum {kraft} != 1 — table incomplete");
    }
}
