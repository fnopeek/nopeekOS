//! Host functions. The shared ones come from `nopeek_widgets::host`; the
//! audio mailbox, the file dialog and the YUV canvas are tune's own.

use nopeek_widgets::host as sdk;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_audio_open() -> i32;
    fn npk_audio_close(slot: i32) -> i32;
    fn npk_audio_submit(slot: i32, ptr: i32, len: i32) -> i32;
    fn npk_audio_buffered(slot: i32) -> i32;
    fn npk_pick(mode: i32, start_ptr: i32, start_len: i32,
                suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32;
    fn npk_canvas_commit_yuv(canvas_id: i32, y_ptr: i32, u_ptr: i32, v_ptr: i32,
                             ys: i32, cs: i32, w: i32, h: i32, flags: i32) -> i32;
}

pub fn scene_commit(bytes: &[u8]) -> bool { sdk::scene_commit(bytes) }
pub fn fetch(name: &str, buf: &mut [u8]) -> Option<usize> { sdk::fetch(name, buf) }
pub fn fs_list(dir: &str, buf: &mut [u8], recursive: bool) -> Option<usize> { sdk::fs_list(dir, buf, recursive) }
pub fn home_dir(buf: &mut [u8]) -> Option<usize> { sdk::home_dir(buf) }
pub fn launch_arg(buf: &mut [u8]) -> Option<usize> { sdk::launch_arg(buf) }
pub fn close_widget() { sdk::close_widget(); }
pub fn ticks() -> i64 { sdk::ticks_ms() as i64 }
pub fn sleep(ms: i32) { sdk::sleep_ms(ms.max(0) as u32); }
pub fn log(msg: &str) { sdk::log_serial(msg); }
pub fn set_volume(pct: i32) { let _ = sdk::audio_set_volume(pct.max(0) as u32); }
/// Master volume in percent, -1 if unknown.
pub fn get_volume() -> i32 {
    sdk::audio_volume().map_or(-1, |v| v.min(i32::MAX as u32) as i32)
}

/// Open the system file dialog. The answer arrives as `Event::Picked`.
pub fn pick_open(start: &str) -> i32 {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_pick(0, start.as_ptr() as i32, start.len() as i32, 0, 0, 0) }
}

pub fn audio_open() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_open() }
}
pub fn audio_close(slot: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_close(slot) };
}
/// Hand `pcm` to the slot's ring; the bytes it accepted.
pub fn audio_submit(slot: i32, pcm: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call and validated by the
    // kernel.
    unsafe { npk_audio_submit(slot, pcm.as_ptr() as i32, pcm.len() as i32) }
}

/// Bytes still sitting in the slot's ring.
///
/// The honest play clock: the wall clock and the audio crystal drift apart,
/// and over a film that shows.
#[allow(dead_code)]
pub fn audio_buffered(slot: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_buffered(slot) }
}

/// Upload a planar 4:2:0 frame. The colour conversion happens in the
/// compositor, at the size the canvas really has — doing it here would cost
/// more than decoding the frame.
///
/// `flags`: bit 0 = Rec. 709, bit 1 = full range.
pub fn canvas_commit_yuv(canvas_id: i32, y: &[u8], u: &[u8], v: &[u8],
                         ys: usize, cs: usize, w: u32, h: u32, flags: i32) -> i32 {
    // SAFETY: FFI; the three planes are borrowed for the call and the kernel
    // validates each against the strides and size.
    unsafe {
        npk_canvas_commit_yuv(canvas_id, y.as_ptr() as i32, u.as_ptr() as i32,
                              v.as_ptr() as i32, ys as i32, cs as i32,
                              w as i32, h as i32, flags)
    }
}
