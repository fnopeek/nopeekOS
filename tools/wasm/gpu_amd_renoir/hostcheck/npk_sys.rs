//! The part of npk_sys the host check needs, on the host: `log` prints
//! the line without the module's prefix, the card is absent.

#[derive(Clone, Copy, Debug)]
pub struct Mmio(pub i32);

impl Mmio {
    pub fn r32(self, _off: u32) -> u32 {
        0
    }
    pub fn w32(self, _off: u32, _v: u32) {}
}

pub fn log(b: &[u8]) {
    let s = String::from_utf8_lossy(b);
    println!("{}", s.strip_prefix("[gpu_amd_renoir] ").unwrap_or(&s));
}

pub fn sys_info(_key: i32) -> i64 {
    0
}

pub fn now_us() -> i64 {
    0
}

pub fn sleep(_ms: i32) -> i32 {
    0
}
