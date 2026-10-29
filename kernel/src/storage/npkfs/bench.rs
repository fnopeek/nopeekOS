//! Where the time of the store's object reads and writes goes, by stage,
//! summed since boot in TSC ticks. `testdisk` reads them before and after
//! a run and shows the difference beside the device's own throughput.

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// One counter set: operations, bytes, and the ticks of each stage.
pub struct Stages<const N: usize> {
    pub count: AtomicU64,
    pub bytes: AtomicU64,
    pub ticks: [AtomicU64; N],
}

impl<const N: usize> Stages<N> {
    const fn new() -> Self {
        Stages { count: AtomicU64::new(0), bytes: AtomicU64::new(0), ticks: [const { AtomicU64::new(0) }; N] }
    }

    /// One operation of `bytes`; `stamps` are the TSC at its start and at
    /// the end of each stage (N + 1 values).
    pub fn add(&self, bytes: u64, stamps: &[u64]) {
        self.count.fetch_add(1, Relaxed);
        self.bytes.fetch_add(bytes, Relaxed);
        for (i, t) in self.ticks.iter().enumerate() {
            if let (Some(a), Some(b)) = (stamps.get(i), stamps.get(i + 1)) {
                t.fetch_add(b.saturating_sub(*a), Relaxed);
            }
        }
    }

    pub fn snapshot(&self, out: &mut alloc::vec::Vec<u64>) {
        out.push(self.count.load(Relaxed));
        out.push(self.bytes.load(Relaxed));
        out.extend(self.ticks.iter().map(|t| t.load(Relaxed)));
    }
}

/// `storage::get`: lock, B-tree lookup, buffer, device read, decrypt.
pub static GET: Stages<5> = Stages::new();
/// `storage::put`: dedup lookup, address hash, encrypt, allocation,
/// device write, B-tree insert.
pub static PUT: Stages<6> = Stages::new();

/// The dedup lookup at the start of `storage::put`: waiting for the store
/// lock, then the B-tree walk.
pub static LOOKUP: Stages<2> = Stages::new();
/// Metadata block reads the cache served, and those it read from the device.
pub static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

/// `fs::write`: the path lock, storing the blob and the trees along the
/// path, the commit.
pub static WRITE: Stages<3> = Stages::new();
/// `storage::commit`: journal, bitmap, cached metadata to the device,
/// device flush, superblock, journal finalize.
pub static COMMIT: Stages<6> = Stages::new();

/// A fast generator for bench data; content must not repeat, but needs no
/// secrecy (xorshift64*).
pub struct Fill(u64);

impl Fill {
    pub fn new(seed: u64) -> Self { Fill(seed | 1) }

    pub fn bytes(&mut self, buf: &mut [u8]) {
        for c in buf.chunks_mut(8) {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
            c.copy_from_slice(&v[..c.len()]);
        }
    }
}
