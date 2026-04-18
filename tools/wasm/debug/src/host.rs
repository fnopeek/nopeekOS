//! Host function bindings for debug.wasm — TCP + terminal mirror + key inject.

unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);

    fn npk_sleep(ms: i32) -> i32;

    fn npk_self_terminal() -> i32;
    fn npk_stream_open(idx: i32) -> i32;
    fn npk_stream_read(idx: i32, buf_ptr: i32, buf_len: i32) -> i32;
    fn npk_stream_close(idx: i32) -> i32;
    fn npk_key_inject(byte: i32) -> i32;

    fn npk_tcp_connect(ip_packed: i32, port: i32) -> i32;
    fn npk_tcp_send(handle: i32, buf_ptr: i32, buf_len: i32) -> i32;
    fn npk_tcp_recv(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_tcp_close(handle: i32) -> i32;

    fn npk_debug_target_ip() -> i32;
    fn npk_debug_target_port() -> i32;
}

pub fn print(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32); }
}

pub fn sleep(ms: i32) { unsafe { npk_sleep(ms); } }

pub fn self_terminal() -> i32 { unsafe { npk_self_terminal() } }
pub fn stream_open(idx: i32) -> i32 { unsafe { npk_stream_open(idx) } }
pub fn stream_read(idx: i32, buf: &mut [u8]) -> i32 {
    unsafe { npk_stream_read(idx, buf.as_mut_ptr() as i32, buf.len() as i32) }
}
pub fn stream_close(idx: i32) { unsafe { npk_stream_close(idx); } }
pub fn key_inject(byte: u8) { unsafe { npk_key_inject(byte as i32); } }

pub fn tcp_connect(ip: i32, port: i32) -> i32 { unsafe { npk_tcp_connect(ip, port) } }
pub fn tcp_send(handle: i32, buf: &[u8]) -> i32 {
    unsafe { npk_tcp_send(handle, buf.as_ptr() as i32, buf.len() as i32) }
}
pub fn tcp_recv(handle: i32, buf: &mut [u8]) -> i32 {
    unsafe { npk_tcp_recv(handle, buf.as_mut_ptr() as i32, buf.len() as i32) }
}
pub fn tcp_close(handle: i32) { unsafe { npk_tcp_close(handle); } }

pub fn target_ip() -> i32 { unsafe { npk_debug_target_ip() } }
pub fn target_port() -> i32 { unsafe { npk_debug_target_port() } }

pub fn print_dec(n: u32) {
    if n >= 10 { print_dec(n / 10); }
    let d = [(n % 10) as u8 + b'0'];
    unsafe { npk_print(d.as_ptr() as i32, 1); }
}

pub fn print_ip(ip: i32) {
    print_dec(((ip >> 24) & 0xFF) as u32); print(".");
    print_dec(((ip >> 16) & 0xFF) as u32); print(".");
    print_dec(((ip >> 8) & 0xFF) as u32); print(".");
    print_dec((ip & 0xFF) as u32);
}
