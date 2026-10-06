//! Safe wrappers for the host calls most apps share.
//!
//! The kernel checks every pointer and length it is handed against the
//! module's memory (`guest`/`guest_mut` in `host_core.rs`); what is unsafe
//! on this side is only the FFI call itself. Each wrapper passes a slice it
//! borrows for exactly the call, so the `unsafe` lives here once instead of
//! at every call site in every app.
//!
//! Return conventions follow the kernel: a length is `Some(n)`, a refusal
//! or error is `None`/`false`.

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_log(ptr: i32, len: i32);
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_print(ptr: i32, len: i32);
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_ticks() -> i64;
    fn npk_unix_time() -> i64;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_spawn_module(ptr: i32, len: i32) -> i32;
    fn npk_run_intent(verb_ptr: i32, verb_len: i32) -> i32;
    fn npk_window_set_modal(modal: i32) -> i32;
    fn npk_window_titles(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_wait(mask: i32, timeout_ms: i32) -> i32;
    fn npk_sys_info(key: i32) -> i64;
    fn npk_screen_size() -> i32;
    fn npk_audio_get_volume() -> i32;
    fn npk_audio_set_volume(pct: i32) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
}

/// A non-negative return as a length.
fn len(n: i32) -> Option<usize> {
    usize::try_from(n).ok()
}

/// To the boot log (`dmesg`); shown on screen only with `set bootlog verbose`.
pub fn log(s: &str) {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_log(s.as_ptr() as i32, s.len() as i32) }
}

/// To the serial port and the remote mirror.
pub fn log_serial(s: &str) {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_log_serial(s.as_ptr() as i32, s.len() as i32) }
}

/// To the terminal the app runs in (or the boot log).
pub fn print(s: &str) {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) }
}

/// Commit an encoded widget tree. False if the kernel refused it.
pub fn scene_commit(bytes: &[u8]) -> bool {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) >= 0 }
}

/// Yield the core for `ms` milliseconds.
pub fn sleep_ms(ms: u32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_sleep(ms.min(i32::MAX as u32) as i32) };
}

/// Milliseconds since boot.
pub fn ticks_ms() -> u64 {
    // SAFETY: FFI without pointers.
    unsafe { npk_ticks() }.max(0) as u64
}

/// Seconds since the Unix epoch (0 if the clock is unknown).
pub fn unix_time() -> u64 {
    // SAFETY: FFI without pointers.
    unsafe { npk_unix_time() }.max(0) as u64
}

/// Read file `name` into `buf`; `Some(n)` bytes copied (a longer file is
/// cut to `buf.len()`).
pub fn fetch(name: &str, buf: &mut [u8]) -> Option<usize> {
    // SAFETY: FFI; both ranges are borrowed for the call and validated by
    // the kernel.
    len(unsafe {
        npk_fetch(name.as_ptr() as i32, name.len() as i32, buf.as_mut_ptr() as i32, buf.len() as i32)
    })
}

/// Write `data` as file `name`. False if refused.
pub fn store(name: &str, data: &[u8]) -> bool {
    // SAFETY: FFI; the kernel validates both ranges.
    unsafe {
        npk_store(name.as_ptr() as i32, name.len() as i32, data.as_ptr() as i32, data.len() as i32) == 0
    }
}

/// List `prefix` into `out` (the kernel's entry encoding); `Some(n)` bytes.
pub fn fs_list(prefix: &str, out: &mut [u8], recursive: bool) -> Option<usize> {
    // SAFETY: FFI; both ranges are borrowed for the call.
    len(unsafe {
        npk_fs_list(prefix.as_ptr() as i32, prefix.len() as i32,
            out.as_mut_ptr() as i32, out.len() as i32, recursive as i32)
    })
}

/// Size, directory flag and mtime of `name`; `None` if it does not exist or
/// may not be read.
pub fn fs_stat(name: &str) -> Option<(u64, bool, u64)> {
    let mut out = [0u8; 17];
    // SAFETY: FFI; `out` is the 17 bytes the kernel writes.
    let r = unsafe { npk_fs_stat(name.as_ptr() as i32, name.len() as i32, out.as_mut_ptr() as i32) };
    if r <= 0 { return None; }
    let size = u64::from_le_bytes(out[0..8].try_into().ok()?);
    let mtime = u64::from_le_bytes(out[9..17].try_into().ok()?);
    Some((size, out[8] != 0, mtime))
}

/// The user's home directory into `buf`; `Some(n)` bytes.
pub fn home_dir(buf: &mut [u8]) -> Option<usize> {
    // SAFETY: FFI; the range is borrowed for the call.
    len(unsafe { npk_home_dir(buf.as_mut_ptr() as i32, buf.len() as i32) })
}

/// The launch argument into `buf`; `Some(0)` when there is none.
pub fn launch_arg(buf: &mut [u8]) -> Option<usize> {
    // SAFETY: FFI; the range is borrowed for the call.
    len(unsafe { npk_launch_arg(buf.as_mut_ptr() as i32, buf.len() as i32) })
}

/// Close this app's window.
pub fn close_widget() {
    // SAFETY: FFI without pointers.
    unsafe { npk_close_widget() };
}

/// Start module `name`. False if refused.
pub fn spawn_module(name: &str) -> bool {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_spawn_module(name.as_ptr() as i32, name.len() as i32) == 0 }
}

/// Run intent `verb` (the kernel's allow-list); its raw result.
pub fn run_intent(verb: &str) -> i32 {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_run_intent(verb.as_ptr() as i32, verb.len() as i32) }
}

/// Make this window modal or not.
pub fn window_set_modal(modal: bool) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_modal(modal as i32) };
}

/// The compositor's window list into `buf`; `Some(n)` bytes.
pub fn window_titles(buf: &mut [u8]) -> Option<usize> {
    // SAFETY: FFI; the range is borrowed for the call.
    len(unsafe { npk_window_titles(buf.as_mut_ptr() as i32, buf.len() as i32) })
}

/// Park until a signal in `mask` or `timeout_ms` (negative: no timeout).
pub fn wait(mask: u32, timeout_ms: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_wait(mask as i32, timeout_ms) }
}

/// One value of the kernel's system information table.
pub fn sys_info(key: i32) -> i64 {
    // SAFETY: FFI without pointers.
    unsafe { npk_sys_info(key) }
}

/// Screen size in pixels, `(0, 0)` without the right to ask.
pub fn screen_size() -> (u32, u32) {
    // SAFETY: FFI without pointers.
    let v = unsafe { npk_screen_size() } as u32;
    (v >> 16, v & 0xFFFF)
}

/// Master volume in percent, or `None`.
pub fn audio_volume() -> Option<u32> {
    // SAFETY: FFI without pointers.
    u32::try_from(unsafe { npk_audio_get_volume() }).ok()
}

/// Set the master volume in percent. False if refused.
pub fn audio_set_volume(pct: u32) -> bool {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_set_volume(pct.min(100) as i32) >= 0 }
}

/// Upload BGRA pixels for canvas `id`. False if refused.
pub fn canvas_commit(id: u32, px: &[u8], w: u32, h: u32) -> bool {
    // SAFETY: FFI; the kernel validates the range against `w * h * 4`.
    unsafe { npk_canvas_commit(id as i32, px.as_ptr() as i32, px.len() as i32, w as i32, h as i32) >= 0 }
}

/// Where canvas `id` was laid out: `(x, y, w, h)` in window space.
pub fn canvas_rect(id: u32) -> Option<(i32, i32, i32, i32)> {
    let mut out = [0u8; 16];
    // SAFETY: FFI; `out` is the 16 bytes the kernel writes.
    if unsafe { npk_canvas_rect(id as i32, out.as_mut_ptr() as i32) } != 0 { return None; }
    let rd = |i: usize| i32::from_le_bytes([out[i], out[i + 1], out[i + 2], out[i + 3]]);
    Some((rd(0), rd(4), rd(8), rd(12)))
}
