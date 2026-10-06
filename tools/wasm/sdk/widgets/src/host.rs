//! Host calls with the conventions an app wants: a length is `Some(n)`, a
//! refusal or error is `None`/`false`. Built on `npk_sys`, which holds the
//! FFI.

use npk_sys as sys;

/// A non-negative return as a length.
fn len(n: i32) -> Option<usize> {
    usize::try_from(n).ok()
}

/// To the boot log (`dmesg`); shown on screen only with `set bootlog verbose`.
pub fn log(s: &str) { sys::log(s.as_bytes()) }

/// To the serial port and the remote mirror.
pub fn log_serial(s: &str) { sys::log_serial(s.as_bytes()) }

/// To the terminal the app runs in (or the boot log).
pub fn print(s: &str) { sys::print(s.as_bytes()) }

/// Commit an encoded widget tree. False if the kernel refused it.
pub fn scene_commit(bytes: &[u8]) -> bool { sys::scene_commit(bytes) >= 0 }

/// Yield the core for `ms` milliseconds.
pub fn sleep_ms(ms: u32) { sys::sleep(ms.min(i32::MAX as u32) as i32); }

/// Milliseconds since boot.
pub fn ticks_ms() -> u64 { sys::ticks().max(0) as u64 }

/// Seconds since the Unix epoch (0 if the clock is unknown).
pub fn unix_time() -> u64 { sys::unix_time().max(0) as u64 }

/// Read file `name` into `buf`; `Some(n)` bytes copied (a longer file is
/// cut to `buf.len()`).
pub fn fetch(name: &str, buf: &mut [u8]) -> Option<usize> { len(sys::fetch(name.as_bytes(), buf)) }

/// Write `data` as file `name`. False if refused.
pub fn store(name: &str, data: &[u8]) -> bool { sys::store(name.as_bytes(), data) == 0 }

/// List `prefix` into `out` (the kernel's entry encoding); `Some(n)` bytes.
pub fn fs_list(prefix: &str, out: &mut [u8], recursive: bool) -> Option<usize> {
    len(sys::fs_list(prefix.as_bytes(), out, recursive))
}

/// Size, directory flag and mtime of `name`; `None` if it does not exist or
/// may not be read.
pub fn fs_stat(name: &str) -> Option<(u64, bool, u64)> {
    let mut out = [0u8; 17];
    if sys::fs_stat(name.as_bytes(), &mut out) <= 0 { return None; }
    let size = u64::from_le_bytes(out[0..8].try_into().ok()?);
    let mtime = u64::from_le_bytes(out[9..17].try_into().ok()?);
    Some((size, out[8] != 0, mtime))
}

/// The user's home directory into `buf`; `Some(n)` bytes.
pub fn home_dir(buf: &mut [u8]) -> Option<usize> { len(sys::home_dir(buf)) }

/// The launch argument into `buf`; `Some(0)` when there is none.
pub fn launch_arg(buf: &mut [u8]) -> Option<usize> { len(sys::launch_arg(buf)) }

/// Close this app's window.
pub fn close_widget() { sys::close_widget(); }

/// Start module `name`. False if refused.
pub fn spawn_module(name: &str) -> bool { sys::spawn_module(name.as_bytes()) == 0 }

/// Run intent `verb` (the kernel's allow-list); its raw result.
pub fn run_intent(verb: &str) -> i32 { sys::run_intent(verb.as_bytes()) }

/// Make this window modal or not.
pub fn window_set_modal(modal: bool) { sys::window_set_modal(modal as i32); }

/// The compositor's window list into `buf`; `Some(n)` bytes.
pub fn window_titles(buf: &mut [u8]) -> Option<usize> { len(sys::window_titles(buf)) }

/// Park until a signal in `mask` or `timeout_ms` (negative: no timeout).
pub fn wait(mask: u32, timeout_ms: i32) -> i32 { sys::wait(mask as i32, timeout_ms) }

/// One value of the kernel's system information table.
pub fn sys_info(key: i32) -> i64 { sys::sys_info(key) }

/// Screen size in pixels, `(0, 0)` without the right to ask.
pub fn screen_size() -> (u32, u32) {
    let v = sys::screen_size() as u32;
    (v >> 16, v & 0xFFFF)
}

/// Master volume in percent, or `None`.
pub fn audio_volume() -> Option<u32> { u32::try_from(sys::audio_get_volume()).ok() }

/// Set the master volume in percent. False if refused.
pub fn audio_set_volume(pct: u32) -> bool { sys::audio_set_volume(pct.min(100) as i32) >= 0 }

/// Upload BGRA pixels for canvas `id`. False if refused.
pub fn canvas_commit(id: u32, px: &[u8], w: u32, h: u32) -> bool {
    sys::canvas_commit(id as i32, px, w as i32, h as i32) >= 0
}

/// Where canvas `id` was laid out: `(x, y, w, h)` in window space.
pub fn canvas_rect(id: u32) -> Option<(i32, i32, i32, i32)> {
    let mut out = [0u8; 16];
    if sys::canvas_rect(id as i32, &mut out) != 0 { return None; }
    let rd = |i: usize| i32::from_le_bytes([out[i], out[i + 1], out[i + 2], out[i + 3]]);
    Some((rd(0), rd(4), rd(8), rd(12)))
}

/// What the system file dialog is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickMode { Open = 0, Save = 1 }

/// Open the system file dialog in `start` (a hint), with `suggest` as the
/// proposed name when saving. The answer arrives as `Event::Picked` with
/// `tag`. False if refused or a dialog for this window is already open.
pub fn pick(mode: PickMode, start: &str, suggest: &str, tag: u32) -> bool {
    sys::pick(mode as i32, start.as_bytes(), suggest.as_bytes(), tag as i32) >= 0
}
