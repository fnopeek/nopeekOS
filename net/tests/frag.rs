//! IPv4 reassembly against Linux's `ip_fragment.c`.

use net_core::frag::{Defrag, Defragged, FRAG_TIME};

const SRC: [u8; 4] = [198, 51, 100, 7];
const DST: [u8; 4] = [192, 0, 2, 1];

/// A fragment of datagram `id`: `data` at byte `offset`, MF as given.
fn frag(id: u16, offset: usize, data: &[u8], mf: bool, tos: u8) -> Vec<u8> {
    let mut ip = vec![0x45, tos, 0, 0, 0, 0, 0, 0, 64, 17, 0, 0];
    ip.extend(SRC);
    ip.extend(DST);
    ip[4..6].copy_from_slice(&id.to_be_bytes());
    let fo = (offset / 8) as u16 | if mf { 0x2000 } else { 0 };
    ip[6..8].copy_from_slice(&fo.to_be_bytes());
    ip.extend(data);
    let len = ip.len() as u16;
    ip[2..4].copy_from_slice(&len.to_be_bytes());
    ip
}

fn payload(n: usize) -> Vec<u8> { (0..n).map(|i| i as u8).collect() }

fn whole(r: Defragged) -> Vec<u8> {
    match r {
        Defragged::Complete(d) => d,
        Defragged::Pending => panic!("still pending"),
        Defragged::Dropped => panic!("dropped"),
    }
}

fn header_ok(d: &[u8]) -> bool {
    let mut sum = 0u32;
    for c in d[..20].chunks(2) { sum += u16::from_be_bytes([c[0], c[1]]) as u32; }
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    sum == 0xffff
}

#[test]
fn in_order_and_reversed_give_the_datagram() {
    let p = payload(3000);
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
        let mut d = Defrag::new();
        let parts = [frag(1, 0, &p[..1480], true, 0), frag(1, 1480, &p[1480..2960], true, 0),
                     frag(1, 2960, &p[2960..], false, 0)];
        let mut out = None;
        for &i in &order {
            match d.input(&parts[i], 0) {
                Defragged::Complete(x) => out = Some(x),
                Defragged::Pending => {}
                Defragged::Dropped => panic!("dropped {order:?}"),
            }
        }
        let x = out.expect("complete");
        assert_eq!(&x[20..], &p[..], "{order:?}");
        assert_eq!(u16::from_be_bytes([x[2], x[3]]) as usize, 3020);
        assert_eq!(u16::from_be_bytes([x[6], x[7]]), 0, "no fragment bits");
        assert!(header_ok(&x));
        assert_eq!(d.mem(), 0);
    }
}

#[test]
fn a_duplicate_is_dropped_alone_an_overlap_drops_the_datagram() {
    let p = payload(2000);
    let mut d = Defrag::new();
    assert!(matches!(d.input(&frag(2, 0, &p[..1000], true, 0), 0), Defragged::Pending));
    assert!(matches!(d.input(&frag(2, 0, &p[..1000], true, 0), 0), Defragged::Dropped));
    let x = whole(d.input(&frag(2, 1000, &p[1000..], false, 0), 0));
    assert_eq!(&x[20..], &p[..]);

    let mut d = Defrag::new();
    d.input(&frag(3, 0, &p[..1000], true, 0), 0);
    assert!(matches!(d.input(&frag(3, 992, &p[992..], false, 0), 0), Defragged::Dropped));
    // The queue is gone: the right fragment alone no longer completes it.
    assert!(matches!(d.input(&frag(3, 1000, &p[1000..], false, 0), 0), Defragged::Pending));
    assert_eq!(d.fails, 1);
}

#[test]
fn a_second_end_that_differs_drops_the_datagram() {
    let p = payload(2000);
    let mut d = Defrag::new();
    d.input(&frag(4, 1000, &p[1000..], false, 0), 0);
    assert!(matches!(d.input(&frag(4, 1000, &p[1000..1500], false, 0), 0), Defragged::Dropped));
    let mut d = Defrag::new();
    d.input(&frag(5, 1000, &p[1000..], false, 0), 0);
    assert!(matches!(d.input(&frag(5, 1008, &p[1008..], true, 0), 0), Defragged::Dropped), "data past the end");
}

#[test]
fn more_than_65535_bytes_is_dropped() {
    let p = payload(1480);
    let mut d = Defrag::new();
    let mut last = Defragged::Pending;
    for k in 0..45 {
        last = d.input(&frag(6, k * 1480, &p, k != 44, 0), 0);
    }
    assert!(matches!(last, Defragged::Dropped));
}

#[test]
fn an_ecn_mix_that_cannot_be_is_dropped_ce_survives() {
    let p = payload(2000);
    let mut d = Defrag::new();
    d.input(&frag(7, 0, &p[..1000], true, 0), 0);
    assert!(matches!(d.input(&frag(7, 1000, &p[1000..], false, 2), 0), Defragged::Dropped));
    let mut d = Defrag::new();
    d.input(&frag(8, 0, &p[..1000], true, 2), 0);
    let x = whole(d.input(&frag(8, 1000, &p[1000..], false, 3), 0));
    assert_eq!(x[1] & 3, 3, "CE carried over");
}

#[test]
fn too_far_apart_the_queue_starts_over() {
    let p = payload(2000);
    let mut d = Defrag::new();
    d.input(&frag(9, 0, &p[..1000], true, 0), 0);
    // 64 fragments of other datagrams from the same source.
    for id in 100..165 { d.input(&frag(id, 0, &p[..8], true, 0), 0); }
    assert!(matches!(d.input(&frag(9, 1000, &p[1000..], false, 0), 0), Defragged::Pending),
        "fragment 0 was thrown away");
}

#[test]
fn an_incomplete_datagram_expires_after_30_s_with_fragment_0() {
    let p = payload(2000);
    let mut d = Defrag::new();
    d.input(&frag(10, 0, &p[..1000], true, 0), 0);
    d.input(&frag(11, 1000, &p[1000..], false, 0), 0);
    assert!(d.expire(FRAG_TIME - 1).is_empty());
    let firsts = d.expire(FRAG_TIME);
    assert_eq!(firsts.len(), 1, "only the queue that had fragment 0");
    assert_eq!(u16::from_be_bytes([firsts[0][4], firsts[0][5]]), 10);
    assert_eq!((d.timeouts, d.mem()), (2, 0));
}
