//! Host calls for testdisk: the shared ones from `nopeek_widgets::host`,
//! plus delete, the benchmark keys of `npk_sys_info` and the layered
//! storage measurements of `npk_disk_bench`.

pub use nopeek_widgets::host::{fetch_into, print, store, Fill};
use nopeek_widgets::host::sys_info;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_fs_delete(name_ptr: i32, name_len: i32) -> i32;
    fn npk_disk_bench(kind: i32, a: i64, b: i64, out_ptr: i32, out_len: i32) -> i32;
}

/// `npk_disk_bench`: the values it wrote into `out`, None when refused or
/// failed. The kinds are documented at the kernel's host call.
pub fn disk_bench(kind: i32, a: u64, b: u64, out: &mut [u64]) -> Option<&[u64]> {
    // SAFETY: FFI; `out` is borrowed for the call and the kernel writes at
    // most its length in bytes.
    let n = unsafe {
        npk_disk_bench(kind, a as i64, b as i64, out.as_mut_ptr() as i32, (out.len() * 8) as i32)
    };
    (n >= 0).then(|| &out[..n as usize])
}

/// Raw TSC ticks (monotonic).
pub fn tsc_now() -> u64 { sys_info(19) as u64 }

/// Crypto probes, kernel-side (AVX2, AES-NI), keys 30..32. The first call
/// measures, the kernel keeps the result until reboot.
pub fn bench_blake3_mbs() -> u64    { sys_info(30) as u64 }
pub fn bench_aes_enc_mbs() -> u64   { sys_info(31) as u64 }
pub fn bench_aes_dec_mbs() -> u64   { sys_info(32) as u64 }

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
