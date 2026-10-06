//! Small runtime pieces every app needs the same way.

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_log(ptr: i32, len: i32);
}

fn log(s: &str) {
    // SAFETY: a valid slice of this module's memory.
    unsafe { npk_log(s.as_ptr() as i32, s.len() as i32) };
}

/// Log a panic's tag and location without allocating; the heap may be the
/// reason for the panic.
pub fn report_panic(tag: &str, info: &core::panic::PanicInfo) {
    log(tag);
    log(" panic");
    if let Some(loc) = info.location() {
        log(" at ");
        log(loc.file());
        let mut digits = [0u8; 10];
        let mut n = loc.line();
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 || i == 0 { break; }
        }
        log(":");
        log(core::str::from_utf8(&digits[i..]).unwrap_or("?"));
    }
}

/// Define the app's `#[panic_handler]`: log, then trap. A trap ends the
/// instance and the kernel cleans up after it; a `loop {}` would pin the
/// core for the rest of the uptime, because fibers are cooperative.
#[macro_export]
macro_rules! panic_handler {
    ($tag:literal) => {
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::rt::report_panic($tag, info);
            core::arch::wasm32::unreachable()
        }
    };
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a
/// character boundary. Slicing typed text at a byte index panics when the
/// index falls inside a character.
pub fn str_clamp(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}
