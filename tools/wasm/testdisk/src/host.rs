//! Host calls for testdisk: the shared ones from `nopeek_widgets::host`,
//! plus delete and the benchmark keys of `npk_sys_info`.

pub use nopeek_widgets::host::{fetch, log, print, store};
use nopeek_widgets::host::sys_info;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_fs_delete(name_ptr: i32, name_len: i32) -> i32;
}

/// Raw TSC ticks (monotonic).
pub fn tsc_now() -> u64 { sys_info(19) as u64 }
/// TSC frequency in MHz.
pub fn tsc_mhz() -> u64 { sys_info(10) as u64 }

/// Bench probes — kernel-side, AVX2/AES-NI/raw-NVMe pathway.
/// Keys 30..34. First call triggers ~100 ms of measurement, results
/// are cached in the kernel until reboot.
pub fn bench_blake3_mbs() -> u64    { sys_info(30) as u64 }
pub fn bench_aes_enc_mbs() -> u64   { sys_info(31) as u64 }
pub fn bench_aes_dec_mbs() -> u64   { sys_info(32) as u64 }
pub fn bench_raw_write_mbs() -> u64 { sys_info(33) as u64 }
pub fn bench_raw_read_mbs() -> u64  { sys_info(34) as u64 }

/// Read-only FS integrity self-check (key 40). Runs the kernel's btree
/// refcount scan now (not cached) and returns the total problem count
/// (0 = clean, -1 = scan error). The detailed report is logged to serial
/// by the kernel. Called at the end of a run so corruption surfaces before
/// a reboot bricks the mount.
pub fn fs_selfcheck() -> i64 { sys_info(40) }

pub fn delete(name: &str) -> bool {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_fs_delete(name.as_ptr() as i32, name.len() as i32) == 0 }
}

/// Decimal print to terminal (no `format!`, no allocation).
pub fn print_dec(n: u64) {
    if n >= 10 { print_dec(n / 10); }
    let d = [(n % 10) as u8 + b'0'];
    print(core::str::from_utf8(&d).unwrap_or("?"));
}
