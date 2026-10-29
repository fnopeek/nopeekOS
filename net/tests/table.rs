//! The connection table: handles, owners and the limits Linux sets in
//! `tcp_init`.

use net_core::tcp::{Env, Stack, Stats, TcpError, KERNEL, PER_OWNER_MAX};

const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const PEER: [u8; 4] = [192, 0, 2, 2];

#[derive(Default)]
struct TestEnv {
    rng: u64,
    sent: Vec<Vec<u8>>,
}

impl Env for TestEnv {
    fn ticks(&self) -> u64 { 0 }
    fn uptime_us(&self) -> u64 { 0 }
    fn our_ip(&self) -> [u8; 4] { OUR_IP }
    fn random_u64(&mut self) -> u64 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.rng >> 16
    }
    fn free_mib(&self) -> usize { 1024 }
    fn arp_target_for(&self, ip: [u8; 4]) -> [u8; 4] { ip }
    fn arp_known(&self, _ip: [u8; 4]) -> bool { true }
    fn arp_request(&mut self, _ip: [u8; 4]) {}
    fn ip_send(&mut self, _dst: [u8; 4], segment: &[u8]) -> bool {
        self.sent.push(segment.to_vec());
        true
    }
}

fn stack(ram_bytes: u64) -> Stack {
    Stack::new([1; 32], Box::leak(Box::new(Stats::new())), ram_bytes)
}

fn flags(seg: &[u8]) -> u8 { seg[13] }

/// The peer's half of a segment to our `port`.
fn segment(port: u16, seq: u32, ack: u32, flags: u8) -> Vec<u8> {
    let mut s = vec![0u8; 20];
    s[0..2].copy_from_slice(&80u16.to_be_bytes());
    s[2..4].copy_from_slice(&port.to_be_bytes());
    s[4..8].copy_from_slice(&seq.to_be_bytes());
    s[8..12].copy_from_slice(&ack.to_be_bytes());
    s[12] = 5 << 4;
    s[13] = flags;
    s[14..16].copy_from_slice(&65535u16.to_be_bytes());
    s
}

/// An established connection: our SYN answered by a SYN-ACK.
fn establish(st: &mut Stack, env: &mut TestEnv) -> (usize, u16, u32) {
    let h = st.connect_start(env, KERNEL, PEER, 80).unwrap();
    let syn = env.sent.pop().unwrap();
    let port = u16::from_be_bytes([syn[0], syn[1]]);
    let isn = u32::from_be_bytes(syn[4..8].try_into().unwrap());
    st.input(env, PEER, &segment(port, 5000, isn.wrapping_add(1), 0x12));
    env.sent.clear();
    assert_eq!(st.connect_status(h), 1);
    (h, port, isn)
}

#[test]
fn grows_past_the_old_table() {
    let mut st = stack(4 << 30);
    let mut env = TestEnv::default();
    let hs: Vec<usize> = (0..300).map(|_| st.connect_start(&mut env, KERNEL, PEER, 80).unwrap()).collect();
    let mut sorted = hs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 300);
    assert!(hs.iter().all(|&h| st.connect_status(h) == 0));
}

#[test]
fn a_stale_handle_never_reaches_the_next_connection() {
    let mut st = stack(4 << 30);
    let mut env = TestEnv::default();
    let old = st.connect_start(&mut env, KERNEL, PEER, 80).unwrap();
    st.close(&mut env, old).unwrap();
    let new = st.connect_start(&mut env, KERNEL, PEER, 80).unwrap();
    assert_eq!(old & 0xffff_ffff, new & 0xffff_ffff, "the place is reused");
    assert_ne!(old, new);
    assert_eq!(st.connect_status(old), -1);
    assert!(matches!(st.close(&mut env, old), Err(TcpError::NotConnected)));
    assert!(matches!(st.send(&mut env, old, b"x"), Err(TcpError::NotConnected)));
    assert_eq!(st.connect_status(new), 0);
}

#[test]
fn a_module_holds_at_most_per_owner_max() {
    let mut st = stack(4 << 30);
    let mut env = TestEnv::default();
    let hs: Vec<usize> = (0..PER_OWNER_MAX).map(|_| st.connect_start(&mut env, 7, PEER, 80).unwrap()).collect();
    assert!(matches!(st.connect_start(&mut env, 7, PEER, 80), Err(TcpError::TooManyConnections)));
    assert!(st.connect_start(&mut env, 8, PEER, 80).is_ok(), "another module is not affected");
    assert!(st.connect_start(&mut env, KERNEL, PEER, 80).is_ok(), "nor the kernel");
    st.close(&mut env, hs[0]).unwrap();
    assert!(st.connect_start(&mut env, 7, PEER, 80).is_ok());
}

#[test]
fn past_max_orphans_close_resets() {
    // 1 MiB of memory: 8 ehash buckets, 4 orphans.
    let mut st = stack(1 << 20);
    let mut env = TestEnv::default();
    let conns: Vec<_> = (0..6).map(|_| establish(&mut st, &mut env)).collect();
    for (i, &(h, _, _)) in conns.iter().enumerate() {
        st.close(&mut env, h).unwrap();
        let sent: Vec<u8> = env.sent.drain(..).map(|s| flags(&s)).collect();
        if i < 4 {
            assert_eq!(sent, [0x11], "orphan {i} sends its FIN");
            assert_eq!(st.connect_status(h), 0, "and lingers in FIN-WAIT-1");
        } else {
            assert_eq!(sent, [0x11, 0x14], "orphan {i} is reset");
            assert_eq!(st.connect_status(h), -1, "and gone");
        }
    }
}

#[test]
fn past_max_tw_buckets_time_wait_is_skipped() {
    let mut st = stack(1 << 20);
    let mut env = TestEnv::default();
    let conns: Vec<_> = (0..4).map(|_| establish(&mut st, &mut env)).collect();
    for &(h, port, isn) in &conns {
        st.close(&mut env, h).unwrap();
        // FIN+ACK from the peer: acknowledges our FIN and closes its side.
        st.input(&mut env, PEER, &segment(port, 5001, isn.wrapping_add(2), 0x11));
    }
    let tw = st.list_connections().iter().filter(|c| c.3 == "TIME_WAIT").count();
    let closed = st.list_connections().iter().filter(|c| c.3 == "CLOSED").count();
    assert_eq!((tw, closed), (4, 0));

    let (h, port, isn) = establish(&mut st, &mut env);
    st.close(&mut env, h).unwrap();
    st.input(&mut env, PEER, &segment(port, 5001, isn.wrapping_add(2), 0x11));
    let tw = st.list_connections().iter().filter(|c| c.3 == "TIME_WAIT").count();
    assert_eq!(tw, 4, "the fifth closes instead of waiting");
}
