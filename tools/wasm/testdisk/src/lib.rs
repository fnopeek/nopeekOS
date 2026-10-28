//! testdisk — the storage stack measured layer by layer.
//!
//! Run as `run testdisk`. Each layer on its own, so the numbers say where
//! the time goes:
//!
//! 1. The device: stored data read back, not data just written (that sits
//!    in the SSD's cache), sequential in several call sizes and 4 KiB at
//!    random places one at a time and many in flight; a write to free
//!    space, flushed. No file system, no crypto.
//! 2. Crypto on one core: BLAKE3, AES-256-GCM.
//! 3. npkFS in the kernel: the largest stored objects read cold, with what
//!    each read spent on lookup, device and decryption.
//! 4. npkFS through the module ABI (`npk_store`, `npk_fetch`): write, read
//!    cold (block cache dropped), read warm; with the same breakdown.
//!
//! Every measurement runs `RUNS` times: minimum, median, maximum. Rates
//! are decimal MB/s, as SSD makers state them. A byte check and the file
//! system self-check close the run. Files go under `.testdisk/`.

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

// Read + exec + render, and WRITE: the device benchmark writes to free
// blocks and the file system check holds the store's lock.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x01 | 0x02 | 0x04 | 0x08]; // READ|WRITE|EXEC|RENDER

mod host;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

// The SDK's growing heap: it frees, and it takes memory from the runtime as
// it is needed instead of reserving it at launch.
#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

const RUNS: usize = 3;
const BLOCK: u64 = 4096;
const MIB: u64 = 1024 * 1024;

/// Stored data read per sequential device run.
const SEQ_READ_BYTES: u64 = 512 * MIB;
/// Data written per device write run.
const SEQ_WRITE_BYTES: u64 = 256 * MIB;
/// Random 4 KiB reads per run: one at a time, and in batches.
const RAND_OPS_QD1: u64 = 2000;
const RAND_OPS_BATCH: u64 = 8192;
const RAND_BATCH: u64 = 32;
/// Stored objects read per kernel file system run.
const OBJECT_READ_BYTES: u64 = 256 * MIB;

/// Module ABI files: (size, files per run, label).
const PLAN: &[(usize, u32, &str)] = &[
    (4 * 1024,         64, "4 KiB"),
    (64 * 1024,        32, "64 KiB"),
    (1024 * 1024,      16, "1 MiB"),
    (16 * 1024 * 1024,  4, "16 MiB"),
    (100 * 1024 * 1024, 1, "100 MiB"),
];

const ROOT: &str = ".testdisk";

/// `npk_disk_bench` kinds.
const SEQ_READ: i32 = 0;
const RAND_READ: i32 = 1;
const SEQ_WRITE: i32 = 2;
const DROP_CACHES: i32 = 3;
const READ_OBJECTS: i32 = 4;
const COUNTERS: i32 = 5;

// ── Output ────────────────────────────────────────────────────────────

fn print(s: &str) { host::print(s); }

fn pad(s: &str, width: usize) -> String {
    let mut out = String::new();
    for _ in s.len()..width { out.push(' '); }
    out.push_str(s);
    out
}

/// "min / median / max" of the values, right-aligned.
fn spread(mut v: Vec<u64>) -> String {
    if v.is_empty() { return String::from("     -"); }
    v.sort_unstable();
    format!("{} / {} / {}", pad(&format!("{}", v[0]), 6), pad(&format!("{}", v[v.len() / 2]), 6),
        pad(&format!("{}", v[v.len() - 1]), 6))
}

fn median(mut v: Vec<u64>) -> u64 {
    if v.is_empty() { return 0; }
    v.sort_unstable();
    v[v.len() / 2]
}

// ── Rates ─────────────────────────────────────────────────────────────

/// Decimal MB/s for `bytes` in `ticks` at `hz`.
fn mbs(bytes: u64, ticks: u64, hz: u64) -> u64 {
    if ticks == 0 { return 0; }
    (bytes as u128 * hz as u128 / ticks as u128 / 1_000_000) as u64
}

fn per_s(ops: u64, ticks: u64, hz: u64) -> u64 {
    if ticks == 0 { return 0; }
    (ops as u128 * hz as u128 / ticks as u128) as u64
}

// ── Stage counters (`npkfs::bench`) ───────────────────────────────────

/// The kernel's counters: object reads (count, bytes, 5 stages), object
/// writes (count, bytes, 6 stages).
#[derive(Clone, Copy, Default)]
struct Counters {
    hz: u64,
    get: [u64; 7],
    put: [u64; 8],
    cmd_blocks: u64,
}

fn counters() -> Option<Counters> {
    let mut out = [0u64; 32];
    let v = host::disk_bench(COUNTERS, 0, 0, &mut out)?;
    if v.len() < 17 { return None; }
    let mut c = Counters { hz: v[0], cmd_blocks: v[16], ..Default::default() };
    c.get.copy_from_slice(&v[1..8]);
    c.put.copy_from_slice(&v[8..16]);
    Some(c)
}

const GET_STAGES: [&str; 5] = ["lock", "lookup", "buffer", "device", "decrypt"];
const PUT_STAGES: [&str; 6] = ["dedup", "hash", "encrypt", "allocate", "device", "tree"];

/// Where the time of `ticks` went: each stage's share between two counter
/// readings, the rest outside the object layer (path layer, copies).
fn breakdown(names: &[&str], before: &[u64], after: &[u64], ticks: u64) -> String {
    let mut s = String::new();
    let mut inside = 0u64;
    for (i, name) in names.iter().enumerate() {
        let d = after[i + 2].saturating_sub(before[i + 2]);
        inside += d;
        let pct = if ticks == 0 { 0 } else { d * 100 / ticks };
        if pct > 0 { s.push_str(&format!("{} {}%  ", name, pct)); }
    }
    let rest = ticks.saturating_sub(inside);
    let pct = if ticks == 0 { 0 } else { rest * 100 / ticks };
    s.push_str(&format!("other {}%", pct));
    s
}

// ── Layer 1: the device ───────────────────────────────────────────────

fn device(hz: u64, cmd_blocks: u64) -> bool {
    print("\n  1  Device: no file system, no crypto      min / median / max\n");
    let mut ok = true;
    let mut sizes = vec![32u64, 256, 2048];
    if cmd_blocks > 0 && !sizes.contains(&cmd_blocks) { sizes.push(cmd_blocks); }
    sizes.sort_unstable();
    for &cmd in &sizes {
        let mut rates = Vec::new();
        let mut read = 0;
        for _ in 0..RUNS {
            let mut out = [0u64; 2];
            match host::disk_bench(SEQ_READ, cmd, SEQ_READ_BYTES / BLOCK, &mut out) {
                Some(&[bytes, t]) => { rates.push(mbs(bytes, t, hz)); read = bytes; }
                _ => ok = false,
            }
        }
        print(&format!("     seq read  {} calls       {} MB/s   {} MiB stored data\n",
            pad(&size_name(cmd * BLOCK), 7), spread(rates), read / MIB));
    }
    for (ops, batch, label) in [(RAND_OPS_QD1, 1, "one at a time"), (RAND_OPS_BATCH, RAND_BATCH, "32 in flight ")] {
        let mut rates = Vec::new();
        for _ in 0..RUNS {
            let mut out = [0u64; 2];
            match host::disk_bench(RAND_READ, ops, batch, &mut out) {
                Some(&[n, t]) => rates.push(per_s(n, t, hz)),
                _ => ok = false,
            }
        }
        print(&format!("     rand read 4 KiB, {}  {} IOPS\n", label, spread(rates)));
    }
    let mut rates = Vec::new();
    let mut written = 0;
    for _ in 0..RUNS {
        let mut out = [0u64; 2];
        match host::disk_bench(SEQ_WRITE, 256, SEQ_WRITE_BYTES / BLOCK, &mut out) {
            Some(&[bytes, t]) => { rates.push(mbs(bytes, t, hz)); written = bytes; }
            _ => ok = false,
        }
    }
    print(&format!("     seq write    1 MiB calls  {} MB/s   {} MiB to free space, flushed\n",
        spread(rates), written / MIB));
    ok
}

fn size_name(bytes: u64) -> String {
    if bytes >= MIB { format!("{} MiB", bytes / MIB) } else { format!("{} KiB", bytes / 1024) }
}

// ── Layer 2: crypto ───────────────────────────────────────────────────

fn crypto() {
    print("\n  2  Crypto, one core\n");
    print(&format!("     BLAKE3 {} MB/s   AES-256-GCM encrypt {} MB/s, decrypt {} MB/s\n",
        host::bench_blake3_mbs(), host::bench_aes_enc_mbs(), host::bench_aes_dec_mbs()));
}

// ── Layer 3: npkFS in the kernel ──────────────────────────────────────

fn objects(hz: u64) -> bool {
    print("\n  3  npkFS in the kernel: largest stored objects, cold\n");
    let mut rates = Vec::new();
    let mut last = None;
    let mut ok = true;
    for _ in 0..RUNS {
        let (Some(before), Some(r)) = (counters(), {
            let mut out = [0u64; 3];
            host::disk_bench(READ_OBJECTS, OBJECT_READ_BYTES, 0, &mut out).map(|v| (v[0], v[1], v[2]))
        }) else {
            ok = false;
            continue;
        };
        let Some(after) = counters() else { ok = false; continue };
        rates.push(mbs(r.1, r.2, hz));
        last = Some((r, before, after));
    }
    if let Some(((n, bytes, t), before, after)) = last {
        print(&format!("     read {} objects, {} MiB   {} MB/s\n", n, bytes / MIB, spread(rates)));
        print(&format!("     time: {}\n", breakdown(&GET_STAGES, &before.get, &after.get, t)));
    }
    ok
}

// ── Layer 4: npkFS through the module ABI ─────────────────────────────

struct FileRun {
    write: u64,
    cold: u64,
    warm: u64,
    put: Option<(Counters, Counters, u64)>,
    get: Option<(Counters, Counters, u64)>,
    failures: u32,
}

fn file_run(size: usize, count: u32, label: &str, wbuf: &mut [u8], rbuf: &mut [u8], seq: &mut u64, hz: u64)
            -> FileRun {
    let name = |i: u32| format!("{}/{}/{:04}", ROOT, label, i);
    let mut run = FileRun { write: 0, cold: 0, warm: 0, put: None, get: None, failures: 0 };
    for i in 0..count { host::delete(&name(i)); }
    let bytes = size as u64 * count as u64;

    // Write: each file distinct, or content addressing stores it once.
    let before = counters();
    let t0 = host::tsc_now();
    for i in 0..count {
        *seq += 1;
        wbuf[..8].copy_from_slice(&seq.to_le_bytes());
        if !host::store(&name(i), &wbuf[..size]) {
            print(&format!("     WARN: store {} failed\n", name(i)));
            run.failures += 1;
        }
    }
    let t = host::tsc_now().wrapping_sub(t0);
    run.write = mbs(bytes, t, hz);
    if let (Some(b), Some(a)) = (before, counters()) { run.put = Some((b, a, t)); }

    // Read cold: the block cache dropped first.
    let _ = host::disk_bench(DROP_CACHES, 0, 0, &mut [0u64; 1]);
    let before = counters();
    let t0 = host::tsc_now();
    for i in 0..count {
        let got = host::fetch_into(&name(i), &mut rbuf[..size + 32]);
        if got != host::Fill::Len(size) {
            print(&format!("     WARN: cold fetch {}: {:?}\n", name(i), got));
            run.failures += 1;
        }
    }
    let t = host::tsc_now().wrapping_sub(t0);
    run.cold = mbs(bytes, t, hz);
    if let (Some(b), Some(a)) = (before, counters()) { run.get = Some((b, a, t)); }

    // Read warm, and check the last file's bytes.
    let t0 = host::tsc_now();
    for i in 0..count {
        let got = host::fetch_into(&name(i), &mut rbuf[..size + 32]);
        if got != host::Fill::Len(size) {
            print(&format!("     WARN: warm fetch {}: {:?}\n", name(i), got));
            run.failures += 1;
        }
    }
    run.warm = mbs(bytes, host::tsc_now().wrapping_sub(t0), hz);
    if rbuf[..size] != wbuf[..size] {
        print(&format!("     WARN: bytes differ after reading back {}\n", label));
        run.failures += 1;
    }

    for i in 0..count { host::delete(&name(i)); }
    run
}

fn files(hz: u64) -> u32 {
    print("\n  4  npkFS through the module ABI: npk_store, npk_fetch          min / median / max\n");
    let max = PLAN.iter().map(|p| p.0).max().unwrap_or(0) + 32;
    let mut wbuf = vec![0u8; max];
    let mut rbuf = vec![0u8; max];
    // Content that does not compress or repeat.
    let mut x = host::tsc_now() | 1;
    for c in wbuf.chunks_mut(8) {
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        c.copy_from_slice(&x.to_le_bytes()[..c.len()]);
    }
    let mut seq = host::tsc_now();
    let mut failures = 0;
    let mut last_big: Option<FileRun> = None;
    for &(size, count, label) in PLAN {
        let runs: Vec<FileRun> = (0..RUNS)
            .map(|_| file_run(size, count, label, &mut wbuf, &mut rbuf, &mut seq, hz))
            .collect();
        failures += runs.iter().map(|r| r.failures).sum::<u32>();
        print(&format!("     {} x {}  write {}  read cold {}  warm {} MB/s\n",
            pad(label, 7), pad(&format!("{}", count), 2),
            spread(runs.iter().map(|r| r.write).collect()),
            spread(runs.iter().map(|r| r.cold).collect()),
            pad(&format!("{}", median(runs.iter().map(|r| r.warm).collect())), 6)));
        last_big = runs.into_iter().last();
    }
    if let Some(r) = last_big {
        if let Some((b, a, t)) = r.put {
            print(&format!("     largest, write time: {}\n", breakdown(&PUT_STAGES, &b.put, &a.put, t)));
        }
        if let Some((b, a, t)) = r.get {
            print(&format!("     largest, cold read time: {}\n", breakdown(&GET_STAGES, &b.get, &a.get, t)));
        }
    }
    failures
}

// ── Entry ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    print(concat!("[testdisk] ", env!("CARGO_PKG_VERSION"), " — the storage stack, layer by layer\n"));
    let Some(c) = counters() else {
        print("[testdisk] the kernel refused the measurements (needs WRITE, a mounted npkFS)\n");
        return;
    };
    if c.hz == 0 {
        print("[testdisk] no TSC frequency, aborting\n");
        return;
    }
    print(&format!("  {} runs each; NVMe commands up to {}\n", RUNS, size_name(c.cmd_blocks * BLOCK)));

    let mut ok = device(c.hz, c.cmd_blocks);
    crypto();
    ok &= objects(c.hz);
    let failures = files(c.hz);

    print("\n  Self-check: ");
    let problems = host::fs_selfcheck();
    match problems {
        0 => print("CLEAN\n"),
        p if p < 0 => print("scan error (FS not mounted?)\n"),
        p => print(&format!("PROBLEMS={}, see the [npk] fsck log for the first one\n", p)),
    }
    if ok && failures == 0 && problems == 0 {
        print("\n[testdisk] ALL OK\n");
    } else {
        print(&format!("\n[testdisk] FAILED: {} file errors, {} fsck problems{}\n", failures,
            problems.max(0), if ok { "" } else { ", a measurement was refused" }));
    }
}
