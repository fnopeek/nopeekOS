//! testdisk — npkFS validation tool.
//!
//! Run as `run testdisk` (no args). Exercises the v2 storage + path
//! layer end-to-end with random data, byte-comparing each read against
//! the bytes that went in. Cleans up after itself.
//!
//! Phases:
//!   A — 50 random files, mixed sizes 16 B…8 KB, write+read+verify
//!   B — 100 small files in one dir, list-and-count
//!   C — single 1 MB blob, write+read+verify (exercises extents)
//!   D — leaf 8 levels deep (exercises path walker)
//!   E — cleanup
//!
//! All paths live under `.testdisk/` so the tool is self-contained
//! and never touches user data. Re-runs are safe: each phase first
//! attempts a delete of any leftovers from a prior aborted run.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

mod host;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

// ── 16 MB bump heap (no free; module exits after one run) ─────────────

const HEAP_SIZE: usize = 16 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAlloc;

unsafe impl core::alloc::GlobalAlloc for BumpAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + align - 1) & !(align - 1);
        if aligned + size > HEAP_SIZE { return core::ptr::null_mut(); }
        unsafe { pos_ptr.write(aligned + size); }
        let heap_ptr = core::ptr::addr_of_mut!(HEAP) as *mut u8;
        unsafe { heap_ptr.add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOC: BumpAlloc = BumpAlloc;

// ── xorshift64 PRNG (deterministic, seedable, no_std-friendly) ────────

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self { Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed }) }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn fill(&mut self, buf: &mut [u8]) {
        let mut i = 0;
        while i + 8 <= buf.len() {
            let v = self.next().to_le_bytes();
            buf[i..i + 8].copy_from_slice(&v);
            i += 8;
        }
        while i < buf.len() {
            buf[i] = (self.next() & 0xFF) as u8;
            i += 1;
        }
    }
}

// ── Counters ──────────────────────────────────────────────────────────

struct Stats {
    writes: u64,
    reads: u64,
    bytes: u64,
    fails: u64,
}

impl Stats {
    const fn new() -> Self { Self { writes: 0, reads: 0, bytes: 0, fails: 0 } }
}

fn print_kv(label: &str, v: u64) {
    host::print(label);
    host::print_dec(v);
    host::print("\n");
}

fn fail(stats: &mut Stats, msg: &str) {
    stats.fails += 1;
    host::print("  FAIL: ");
    host::print(msg);
    host::print("\n");
}

// ── Per-phase routines ────────────────────────────────────────────────

const TEST_ROOT: &str = ".testdisk";
const N_RANDOM: usize = 50;
const MAX_RANDOM_SIZE: usize = 8 * 1024;
const N_DIR_FILES: usize = 100;
const LARGE_BLOB_SIZE: usize = 1024 * 1024;

fn phase_a_random_roundtrip(rng: &mut Rng, stats: &mut Stats) {
    host::print("\n[testdisk] Phase A: 50 random files, write+read+verify\n");
    let mut written: Vec<(String, Vec<u8>)> = Vec::with_capacity(N_RANDOM);

    for i in 0..N_RANDOM {
        let size = ((rng.next() as usize) % MAX_RANDOM_SIZE) + 16;
        let mut payload = vec![0u8; size];
        rng.fill(&mut payload);
        let name = format!("{}/rand/{:03}", TEST_ROOT, i);
        host::delete(&name); // best-effort cleanup of leftover
        if !host::store(&name, &payload) {
            fail(stats, &format!("store {}", name));
            continue;
        }
        stats.writes += 1;
        stats.bytes += size as u64;
        written.push((name, payload));
    }

    let mut buf = vec![0u8; MAX_RANDOM_SIZE + 32];
    for (name, expected) in &written {
        let n = host::fetch(name, &mut buf);
        if n < 0 {
            fail(stats, &format!("fetch {}", name));
            continue;
        }
        let actual = &buf[..n as usize];
        if actual != expected.as_slice() {
            fail(stats, &format!("mismatch {} ({} bytes returned, expected {})",
                name, n, expected.len()));
            continue;
        }
        stats.reads += 1;
    }

    host::print("  ok: ");
    host::print_dec(written.len() as u64);
    host::print(" written, ");
    host::print_dec(stats.reads);
    host::print(" verified\n");

    // Stash names in stats? No — clean up here so we don't carry state.
    for (name, _) in &written {
        host::delete(name);
    }
}

fn phase_b_many_in_dir(rng: &mut Rng, stats: &mut Stats) {
    host::print("\n[testdisk] Phase B: 100 files in one dir, list-and-count\n");

    for i in 0..N_DIR_FILES {
        let mut data = [0u8; 64];
        rng.fill(&mut data);
        let name = format!("{}/many/f{:03}", TEST_ROOT, i);
        host::delete(&name);
        if !host::store(&name, &data) {
            fail(stats, &format!("store {}", name));
            continue;
        }
        stats.writes += 1;
        stats.bytes += 64;
    }

    // List the dir; entries are separated by `\n`, count them.
    let mut list_buf = vec![0u8; 32 * 1024];
    let prefix = format!("{}/many", TEST_ROOT);
    let n = host::fs_list(&prefix, &mut list_buf, false);
    if n <= 0 {
        fail(stats, "fs_list returned no entries");
    } else {
        let count = list_buf[..n as usize].iter().filter(|&&b| b == b'\n').count() + 1;
        host::print("  listed ");
        host::print_dec(count as u64);
        host::print(" entries (expected ");
        host::print_dec(N_DIR_FILES as u64);
        host::print(")");
        if count != N_DIR_FILES {
            host::print(" — MISMATCH\n");
            stats.fails += 1;
        } else {
            host::print(" — ok\n");
        }
    }

    // Stat one of them to verify the kind/size encoding.
    let probe = format!("{}/many/f042", TEST_ROOT);
    let mut sbuf = [0u8; 9];
    let r = host::fs_stat(&probe, &mut sbuf);
    if r == 9 {
        let size = u64::from_le_bytes([
            sbuf[0], sbuf[1], sbuf[2], sbuf[3], sbuf[4], sbuf[5], sbuf[6], sbuf[7],
        ]);
        let is_dir = sbuf[8];
        host::print("  stat probe: size=");
        host::print_dec(size);
        host::print(" is_dir=");
        host::print_dec(is_dir as u64);
        if size != 64 || is_dir != 0 {
            host::print(" — MISMATCH\n");
            stats.fails += 1;
        } else {
            host::print(" — ok\n");
        }
    } else {
        fail(stats, &format!("fs_stat returned {}", r));
    }

    // Cleanup
    for i in 0..N_DIR_FILES {
        host::delete(&format!("{}/many/f{:03}", TEST_ROOT, i));
    }
}

fn phase_c_large_blob(rng: &mut Rng, stats: &mut Stats) {
    host::print("\n[testdisk] Phase C: 1 MB single blob, write+read+verify\n");

    let mut large = vec![0u8; LARGE_BLOB_SIZE];
    rng.fill(&mut large);
    let name = format!("{}/large.bin", TEST_ROOT);
    host::delete(&name);

    if !host::store(&name, &large) {
        fail(stats, "store large blob");
        return;
    }
    stats.writes += 1;
    stats.bytes += LARGE_BLOB_SIZE as u64;

    let mut buf = vec![0u8; LARGE_BLOB_SIZE + 32];
    let n = host::fetch(&name, &mut buf);
    if n != LARGE_BLOB_SIZE as i32 {
        fail(stats, &format!("fetch returned {} bytes (expected {})", n, LARGE_BLOB_SIZE));
    } else if &buf[..LARGE_BLOB_SIZE] != large.as_slice() {
        fail(stats, "1 MB content mismatch");
    } else {
        stats.reads += 1;
        host::print("  ok: 1 MB roundtrip clean\n");
    }

    host::delete(&name);
}

fn phase_d_deep_nesting(stats: &mut Stats) {
    host::print("\n[testdisk] Phase D: 8-level deep file path\n");

    let name = format!("{}/deep/a/b/c/d/e/f/g/leaf", TEST_ROOT);
    let payload = b"deep!";
    host::delete(&name);

    if !host::store(&name, payload) {
        fail(stats, "store deep");
        return;
    }
    stats.writes += 1;
    stats.bytes += payload.len() as u64;

    let mut buf = [0u8; 16];
    let n = host::fetch(&name, &mut buf);
    if n != payload.len() as i32 || &buf[..payload.len()] != payload.as_slice() {
        fail(stats, &format!("deep mismatch (n={})", n));
    } else {
        stats.reads += 1;
        host::print("  ok: 8-level walk + fetch clean\n");
    }

    host::delete(&name);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[testdisk] starting (npkFS v2 validation)\n");

    let mut rng = Rng::new(0x12345678);
    let mut stats = Stats::new();

    phase_a_random_roundtrip(&mut rng, &mut stats);
    phase_b_many_in_dir(&mut rng, &mut stats);
    phase_c_large_blob(&mut rng, &mut stats);
    phase_d_deep_nesting(&mut stats);

    host::print("\n[testdisk] summary:\n");
    print_kv("  writes:   ", stats.writes);
    print_kv("  reads:    ", stats.reads);
    print_kv("  bytes:    ", stats.bytes);
    print_kv("  failures: ", stats.fails);
    if stats.fails == 0 {
        host::print("\n[testdisk] ALL OK — npkFS v2 round-trips clean.\n");
    } else {
        host::print("\n[testdisk] FAILED — see lines above.\n");
    }
}
