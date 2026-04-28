//! testdisk — npkFS v2 benchmark + roundtrip validation.
//!
//! Run as `run testdisk`. For each size bucket (256 B, 4 KB, 64 KB,
//! 1 MB) we time WRITE then READ over `count` ops, report IOPS +
//! throughput, then delete to clean up. A small roundtrip check at
//! the end byte-compares one read per bucket so a silent corruption
//! shows up immediately.
//!
//! All paths under `.testdisk/`. Re-runs are safe (each store path
//! is delete-first).

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

// ── 256 MB bump heap ──────────────────────────────────────────────────
//
// Sized for the 100 MB bucket: 100 MB write_buf + 100 MB read_buf + ~50
// MB slack for transient kernel-side allocations (storage::put alloc'd
// AES-GCM ciphertext + tree-rebuild + commit-journal scratch).
//
// 256 MB as bss adds nothing to the binary on disk (zero-init), but
// the wasmi runtime has to back it with real pages on instantiate, so
// startup gets slower — measured ~50 ms extra on N100. Acceptable for
// a one-shot bench.
const HEAP_SIZE: usize = 256 * 1024 * 1024;
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

// ── Output formatting (no f64 in WASM no_std — keep integer-only) ─────

fn print_dec(n: u64) { host::print_dec(n); }

fn print_pad(n: u64, width: usize) {
    // Right-align decimal `n` in `width` columns. Used for table rows.
    let mut tmp = [0u8; 24];
    let mut i = tmp.len();
    let mut x = n;
    if x == 0 {
        i -= 1; tmp[i] = b'0';
    } else {
        while x > 0 {
            i -= 1;
            tmp[i] = b'0' + (x % 10) as u8;
            x /= 10;
        }
    }
    let len = tmp.len() - i;
    for _ in 0..width.saturating_sub(len) { host::print(" "); }
    let s = core::str::from_utf8(&tmp[i..]).unwrap_or("?");
    host::print(s);
}

// ── Size formatting ──────────────────────────────────────────────────

fn fmt_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{} B ", bytes)
    }
}

// ── Benchmark plan ────────────────────────────────────────────────────

/// (size_bytes, count_per_phase, label_prefix) — kept small so a slow
/// WASM interpreter doesn't spend forever on the unmeasured loop overhead.
/// We're measuring kernel FS perf, not Rust→Wasm code-gen.
///
/// `xhuge` (16 MB × 1) probes the data-bandwidth ceiling: at this size
/// the indirect-extent chain has thousands of entries, and AEAD +
/// BLAKE3 are running over real bytes — what we measure here is the
/// NVMe queue-depth-1 bottleneck, not the FS-layer commit overhead.
const PLAN: &[(usize, u32, &str)] = &[
    (256,                  50, "small"),
    (4 * 1024,             20, "medium"),
    (64 * 1024,            10, "large"),
    (1024 * 1024,           4, "huge"),
    (16 * 1024 * 1024,      1, "xhuge"),
    (100 * 1024 * 1024,     1, "100M"),
];

const ROOT: &str = ".testdisk";

// ── Per-phase timer ───────────────────────────────────────────────────

struct PhaseStats {
    label: &'static str,
    size: usize,
    count: u32,
    write_us: u64,
    read_us: u64,
    failures: u32,
}

fn ticks_to_us(ticks: u64, tsc_mhz: u64) -> u64 {
    if tsc_mhz == 0 { return 0; }
    ticks / tsc_mhz
}

fn iops(count: u32, us: u64) -> u64 {
    if us == 0 { return 0; }
    count as u64 * 1_000_000 / us
}

/// KB/s = bytes × 1000 / µs (decimal KB, close enough). KB rather
/// than MB so small-write rows don't round to zero from integer math.
fn kb_per_s(total_bytes: u64, us: u64) -> u64 {
    if us == 0 { return 0; }
    total_bytes.saturating_mul(1000) / us
}

// ── Phase runner ──────────────────────────────────────────────────────

fn run_phase(
    label: &'static str,
    size: usize,
    count: u32,
    write_buf: &mut [u8],
    read_buf: &mut [u8],
    tsc_mhz: u64,
) -> PhaseStats {
    let mut stats = PhaseStats {
        label, size, count, write_us: 0, read_us: 0, failures: 0,
    };

    // Pre-clean any leftover from prior runs (cheap on a fresh fs).
    for i in 0..count {
        let name = format!("{}/{}/{:04}", ROOT, label, i);
        host::delete(&name);
    }

    // ── WRITE ────────────────────────────────────────────────────────
    let t0 = host::tsc_now();
    for i in 0..count {
        // Tweak the first 8 bytes per iteration so each blob has a
        // distinct hash — without that, the storage layer's content
        // dedup makes 2..N writes free and the throughput number lies.
        let counter = i as u64;
        write_buf[..8].copy_from_slice(&counter.to_le_bytes());

        let name = format!("{}/{}/{:04}", ROOT, label, i);
        if !host::store(&name, &write_buf[..size]) {
            stats.failures += 1;
        }
    }
    let t1 = host::tsc_now();
    stats.write_us = ticks_to_us(t1.wrapping_sub(t0), tsc_mhz);

    // ── READ ─────────────────────────────────────────────────────────
    let t2 = host::tsc_now();
    for i in 0..count {
        let name = format!("{}/{}/{:04}", ROOT, label, i);
        let n = host::fetch(&name, &mut read_buf[..size + 32]);
        if n != size as i32 {
            stats.failures += 1;
        }
    }
    let t3 = host::tsc_now();
    stats.read_us = ticks_to_us(t3.wrapping_sub(t2), tsc_mhz);

    // ── Roundtrip check on one entry per phase (catches silent corrupt) ─
    let counter = 0u64;
    write_buf[..8].copy_from_slice(&counter.to_le_bytes());
    let probe_name = format!("{}/{}/{:04}", ROOT, label, 0);
    let n = host::fetch(&probe_name, &mut read_buf[..size + 32]);
    if n == size as i32 {
        if &read_buf[..size] != &write_buf[..size] {
            host::print("  WARN: byte mismatch on roundtrip probe — ");
            host::print(label);
            host::print("\n");
            stats.failures += 1;
        }
    }

    // Cleanup
    for i in 0..count {
        let name = format!("{}/{}/{:04}", ROOT, label, i);
        host::delete(&name);
    }

    stats
}

fn print_phase(s: &PhaseStats) {
    host::print("  ");
    host::print(s.label);
    for _ in s.label.len()..8 { host::print(" "); }
    host::print(&fmt_size(s.size));
    host::print("  ");

    print_pad(s.count as u64, 4);
    host::print(" ops  |  WRITE ");
    print_pad(iops(s.count, s.write_us), 6);
    host::print(" iops, ");
    print_pad(kb_per_s(s.size as u64 * s.count as u64, s.write_us), 6);
    host::print(" KB/s  |  READ ");
    print_pad(iops(s.count, s.read_us), 6);
    host::print(" iops, ");
    print_pad(kb_per_s(s.size as u64 * s.count as u64, s.read_us), 6);
    host::print(" KB/s");

    if s.failures > 0 {
        host::print("  |  FAIL ");
        print_pad(s.failures as u64, 0);
    }
    host::print("\n");
}

// ── Entry ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Direct-to-serial so the timestamp shows up the instant WASM
    // starts executing. Compare with the kernel's "Running ..." log
    // — gap between them = wasmi instantiate cost.
    host::log("[testdisk] _start entry");

    host::print("[testdisk] npkFS v2 benchmark\n");
    let tsc_mhz = host::tsc_mhz();
    host::print("  TSC: ");
    print_dec(tsc_mhz);
    host::print(" MHz\n\n");

    if tsc_mhz == 0 {
        host::print("[testdisk] no TSC frequency available, aborting.\n");
        return;
    }

    host::print("  alloc start, ticks_us_total=");
    let t_alloc0 = host::tsc_now();

    // 100 MB buffers (sized for the 100M bucket). Rust's `vec![0; N]`
    // memsets which on N100 = ~30 ms × 2 buffers; wasmi's
    // `memory.fill` lowers to host memset so it's near-DRAM-bandwidth.
    let max_size = 100 * 1024 * 1024 + 32;
    let mut write_buf: Vec<u8> = vec![0u8; max_size];
    let mut read_buf: Vec<u8> = vec![0u8; max_size];

    let t_alloc1 = host::tsc_now();
    print_dec((t_alloc1.wrapping_sub(t_alloc0)) / tsc_mhz);
    host::print(" us\n");

    host::print("  size      ops  |  WRITE              |  READ\n");
    host::print("  ──────────────────────────────────────────────────────────\n");

    let mut all = Vec::with_capacity(PLAN.len());
    for &(size, count, label) in PLAN {
        let s = run_phase(label, size, count, &mut write_buf, &mut read_buf, tsc_mhz);
        print_phase(&s);
        all.push(s);
    }

    // Aggregate
    let total_bytes: u64 = all.iter().map(|s| s.size as u64 * s.count as u64).sum();
    let total_writes: u32 = all.iter().map(|s| s.count).sum();
    let total_write_us: u64 = all.iter().map(|s| s.write_us).sum();
    let total_read_us: u64 = all.iter().map(|s| s.read_us).sum();
    let total_fails: u32 = all.iter().map(|s| s.failures).sum();

    host::print("\n  totals:    ");
    print_pad(total_writes as u64, 4);
    host::print(" ops  |  WRITE ");
    print_pad(iops(total_writes, total_write_us), 6);
    host::print(" iops, ");
    print_pad(kb_per_s(total_bytes, total_write_us), 6);
    host::print(" KB/s  |  READ ");
    print_pad(iops(total_writes, total_read_us), 6);
    host::print(" iops, ");
    print_pad(kb_per_s(total_bytes, total_read_us), 6);
    host::print(" KB/s\n");

    host::print("\n  bytes touched: ");
    print_dec(total_bytes);
    host::print("  (");
    print_dec(total_bytes / (1024 * 1024));
    host::print(" MB)\n");

    // Ceiling probes — what the HW + crypto can do in isolation, vs
    // the FS-stack throughput above. First sys_info(30..) call
    // triggers the bench; the kernel caches the result until reboot.
    host::print("\n  Crypto:    BLAKE3 ");
    print_dec(host::bench_blake3_mbs());
    host::print(" MB/s | AES-GCM enc ");
    print_dec(host::bench_aes_enc_mbs());
    host::print(" dec ");
    print_dec(host::bench_aes_dec_mbs());
    host::print(" MB/s\n");

    host::print("  Raw NVMe:  WRITE ");
    print_dec(host::bench_raw_write_mbs());
    host::print(" MB/s, READ ");
    print_dec(host::bench_raw_read_mbs());
    host::print(" MB/s  (1 MB extent, no FS, no crypto)\n");

    if total_fails == 0 {
        host::print("\n[testdisk] ALL OK\n");
    } else {
        host::print("\n[testdisk] FAILED — ");
        print_dec(total_fails as u64);
        host::print(" errors\n");
    }
}
