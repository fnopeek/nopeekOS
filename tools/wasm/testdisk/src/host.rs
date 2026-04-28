//! Host function bindings for testdisk.wasm.

unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);

    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32,
                   out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
    fn npk_fs_delete(name_ptr: i32, name_len: i32) -> i32;
}

pub fn print(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32); }
}

/// Strict create — fails if `name` already exists. Caller is expected
/// to delete first if overwrite is desired.
pub fn store(name: &str, data: &[u8]) -> bool {
    unsafe {
        npk_store(name.as_ptr() as i32, name.len() as i32,
                  data.as_ptr() as i32, data.len() as i32) == 0
    }
}

/// Returns bytes written into `buf` on success, -1 on error / not found.
pub fn fetch(name: &str, buf: &mut [u8]) -> i32 {
    unsafe {
        npk_fetch(name.as_ptr() as i32, name.len() as i32,
                  buf.as_mut_ptr() as i32, buf.len() as i32)
    }
}

/// Returns bytes written into `buf`. 0 if the prefix is empty / has no
/// children, -1 on error.
pub fn fs_list(prefix: &str, buf: &mut [u8], recursive: bool) -> i32 {
    unsafe {
        npk_fs_list(prefix.as_ptr() as i32, prefix.len() as i32,
                    buf.as_mut_ptr() as i32, buf.len() as i32,
                    if recursive { 1 } else { 0 })
    }
}

/// 9 → wrote (size_u64 + is_dir_u8); 0 → not found; -1 → error.
pub fn fs_stat(name: &str, out: &mut [u8; 9]) -> i32 {
    unsafe {
        npk_fs_stat(name.as_ptr() as i32, name.len() as i32, out.as_mut_ptr() as i32)
    }
}

pub fn delete(name: &str) -> bool {
    unsafe { npk_fs_delete(name.as_ptr() as i32, name.len() as i32) == 0 }
}

/// Decimal print to terminal (no `format!`, no allocation).
pub fn print_dec(n: u64) {
    if n >= 10 { print_dec(n / 10); }
    let d = [(n % 10) as u8 + b'0'];
    unsafe { npk_print(d.as_ptr() as i32, 1); }
}
