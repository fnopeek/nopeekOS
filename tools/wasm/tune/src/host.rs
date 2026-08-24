//! Host functions — WASM imports from the `env` module, resolved by the
//! kernel at instantiation. Naming the module explicitly is what makes them
//! imports rather than ordinary undefined C symbols, which rust-lld rejects.

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_ticks() -> i64;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_audio_open() -> i32;
    fn npk_audio_close(slot: i32) -> i32;
    fn npk_audio_submit(slot: i32, ptr: i32, len: i32) -> i32;
    fn npk_audio_set_volume(pct: i32) -> i32;
    fn npk_audio_get_volume() -> i32;
}

pub fn scene_commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}
pub fn event_poll(ptr: *mut u8, max: usize) -> i32 {
    unsafe { npk_event_poll(ptr as i32, max as i32) }
}
pub fn fetch(name: &str, buf: *mut u8, max: usize) -> i32 {
    unsafe { npk_fetch(name.as_ptr() as i32, name.len() as i32, buf as i32, max as i32) }
}
pub fn fs_list(dir: &str, buf: *mut u8, max: usize) -> i32 {
    unsafe { npk_fs_list(dir.as_ptr() as i32, dir.len() as i32, buf as i32, max as i32, 0) }
}
pub fn home_dir(buf: *mut u8, max: usize) -> i32 {
    unsafe { npk_home_dir(buf as i32, max as i32) }
}
pub fn launch_arg(buf: *mut u8, max: usize) -> i32 {
    unsafe { npk_launch_arg(buf as i32, max as i32) }
}
pub fn close_widget() { unsafe { let _ = npk_close_widget(); } }
pub fn ticks() -> i64 { unsafe { npk_ticks() } }
pub fn sleep(ms: i32) { unsafe { let _ = npk_sleep(ms); } }
pub fn log(msg: &str) { unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32) } }

pub fn audio_open() -> i32 { unsafe { npk_audio_open() } }
pub fn audio_close(slot: i32) { unsafe { let _ = npk_audio_close(slot); } }
pub fn audio_submit(slot: i32, ptr: i32, len: i32) -> i32 { unsafe { npk_audio_submit(slot, ptr, len) } }
pub fn set_volume(pct: i32) { unsafe { let _ = npk_audio_set_volume(pct); } }
pub fn get_volume() -> i32 { unsafe { npk_audio_get_volume() } }
