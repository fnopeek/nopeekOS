//! top — nopeekOS system monitor (WASM module)
//!
//! Live-updating display: per-core CPU usage, frequency, memory, scheduler.
//! Uses the App Display API: npk_print, npk_clear, npk_input_wait, npk_sys_info.
//! Press 'q' to exit.

#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

// ── App Display API ──────────────────────────────────────────────

unsafe extern "C" {
    /// Write text to the app's display area.
    fn npk_print(ptr: i32, len: i32);
    /// Clear the app's display.
    fn npk_clear();
    /// Wait for a key press or timeout. Returns key (0-255) or -1 (timeout).
    fn npk_input_wait(timeout_ms: i32) -> i32;
    /// Query system information.
    fn npk_sys_info(key: i32) -> i64;
}

fn print(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32); }
}

fn sys(key: i32) -> i64 {
    unsafe { npk_sys_info(key) }
}

fn print_num(n: i64) {
    if n < 0 { print("-"); print_num(-n); return; }
    if n >= 10 { print_num(n / 10); }
    let d = (n % 10) as u8 + b'0';
    let s = [d];
    unsafe { npk_print(s.as_ptr() as i32, 1); }
}

fn pad(n: i64, w: usize) {
    let mut digits = 1usize;
    let mut t = if n > 0 { n } else { 1 };
    while t >= 10 { digits += 1; t /= 10; }
    let mut p = if w > digits { w - digits } else { 0 };
    while p > 0 { print(" "); p -= 1; }
    print_num(n);
}

fn bar(pct: i64, w: usize) {
    let f = (pct as usize * w / 100).min(w);
    print("[");
    let mut i = 0;
    while i < f { print("|"); i += 1; }
    i = 0;
    while i < w - f { print(" "); i += 1; }
    print("]");
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    loop {
        unsafe { npk_clear(); }

        let cores = sys(0);
        let uptime = sys(1);
        let free_mb = sys(2);
        let heap_used = sys(3);
        let heap_total = sys(4);
        let spawned = sys(5);
        let completed = sys(6);
        let steals = sys(7);
        let workers = sys(8);
        let has_mwait = sys(9);
        let tsc_mhz = sys(10);
        let max_turbo = sys(13);
        let min_eff = sys(14);

        // Header
        print("\n  nopeekOS top — ");
        print_num(cores);
        print(" cores, up ");
        let h = uptime / 3600;
        let m = (uptime % 3600) / 60;
        let s = uptime % 60;
        if h > 0 { print_num(h); print("h "); }
        print_num(m); print("m "); print_num(s); print("s\n");

        print("  CPU: ");
        if max_turbo > 0 {
            print_num(min_eff); print("-"); print_num(max_turbo); print(" MHz (HWP)");
        } else {
            print("TSC "); print_num(tsc_mhz); print(" MHz");
        }
        print("  ["); print_num(workers); print(" workers, ");
        if has_mwait != 0 { print("MWAIT"); } else { print("HLT"); }
        print("]\n");
        print("  ──────────────────────────────────────────────────\n\n");

        // Per-core
        print("  CORE  USAGE    MHz  QUEUE  ROLE\n");
        print("  ────  ─────  ─────  ─────  ────\n");

        let mut i: i64 = 0;
        while i < cores {
            print("  ");
            pad(i, 4);

            let usage = sys(15 | ((i as i32) << 8) as i32);
            print("  ");
            pad(usage, 3);
            print("%");

            let mhz = sys(12 | ((i as i32) << 8) as i32);
            print("  ");
            pad(mhz, 5);

            let qlen = sys(11 | ((i as i32) << 8) as i32);
            print("  ");
            pad(qlen, 5);

            if i == 0 {
                print("  kernel/irq");
            } else if usage > 5 {
                print("  worker");
            } else {
                print("  idle");
            }
            print("\n");
            i += 1;
        }

        // Memory
        print("\n  MEMORY\n  ──────\n");
        print("  RAM:  "); print_num(free_mb); print(" MB free\n");
        print("  Heap: "); print_num(heap_used / 1024); print(" KB / ");
        print_num(heap_total / (1024 * 1024)); print(" MB ");
        bar(heap_used * 100 / heap_total.max(1), 20);
        print("\n");

        // Scheduler
        print("\n  SCHED  spawned="); print_num(spawned);
        print("  done="); print_num(completed);
        print("  steals="); print_num(steals); print("\n");

        // Debug
        let c15_eax = sys(16);
        let c15_ebx = sys(17);
        let c15_ecx = sys(18);
        print("\n  TSC: "); print_num(tsc_mhz); print(" MHz");
        print("  (CPUID 0x15: "); print_num(c15_ecx);
        print("*"); print_num(c15_ebx);
        print("/"); print_num(c15_eax); print(")\n");

        print("  [q] quit\n");

        // Wait for key or 1-second timeout — instant response to 'q'
        let key = unsafe { npk_input_wait(1000) };
        if key == 0x71 || key == 0x51 { return; } // 'q' or 'Q'
    }
}
