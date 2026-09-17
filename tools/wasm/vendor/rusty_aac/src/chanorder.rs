//! Kanalreihenfolge AAC -> verschachtelt.
//!
//! Stand in `encode.rs` und wird von `decode.rs` gebraucht — der Encoder ist
//! bei uns abgeschaltet, also liegt die Funktion hier. Woertlich dieselbe.
use alloc::vec;
use alloc::vec::Vec;

pub(crate) fn aac_to_interleave_order(channels: usize) -> Option<Vec<usize>> {
    Some(match channels {
        1 => vec![0],
        2 => vec![0, 1],
        // AAC order C,L,R -> interleave L,R,C
        3 => vec![1, 2, 0],
        // AAC order C,L,R,Cs -> interleave L,R,C,Cs
        4 => vec![1, 2, 0, 3],
        // AAC order C,L,R,Ls,Rs -> interleave L,R,C,Ls,Rs
        5 => vec![1, 2, 0, 3, 4],
        // AAC order C,L,R,Ls,Rs,LFE -> interleave L,R,C,LFE,Ls,Rs
        6 => vec![1, 2, 0, 5, 3, 4],
        _ => return None,
    })
}
