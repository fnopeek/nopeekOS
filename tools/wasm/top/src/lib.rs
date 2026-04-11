//! top — nopeekOS system monitor (WASM module)
//!
//! Live-updating display: per-core frequency, memory, scheduler stats.
//! Runs in interactive mode — npk_print writes directly to terminal.
//! Press 'q' to exit.

#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

// --- Host function bindings ---

unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);
    fn npk_sys_info(key: i32) -> i64;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_input_poll() -> i32;
    fn npk_clear();
}

fn print(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32); }
}

fn sys(key: i32) -> i64 {
    unsafe { npk_sys_info(key) }
}

fn print_num(n: i64) {
    if n < 0 {
        print("-");
        print_num(-n);
        return;
    }
    if n >= 10 {
        print_num(n / 10);
    }
    let digit = (n % 10) as u8 + b'0';
    let s = [digit];
    unsafe { npk_print(s.as_ptr() as i32, 1); }
}

fn print_num_padded(n: i64, width: usize) {
    let mut digits = 1usize;
    let mut tmp = if n > 0 { n } else { 1 };
    while tmp >= 10 { digits += 1; tmp /= 10; }
    let mut pad = if width > digits { width - digits } else { 0 };
    while pad > 0 { print(" "); pad -= 1; }
    print_num(n);
}

fn print_bar(used: i64, total: i64, width: usize) {
    let filled = if total > 0 { (used * width as i64 / total) as usize } else { 0 };
    let empty = width.saturating_sub(filled);
    print("[");
    let mut i = 0;
    while i < filled { print("|"); i += 1; }
    i = 0;
    while i < empty { print(" "); i += 1; }
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
        let hours = uptime / 3600;
        let mins = (uptime % 3600) / 60;
        let secs = uptime % 60;
        if hours > 0 {
            print_num(hours);
            print("h ");
        }
        print_num(mins);
        print("m ");
        print_num(secs);
        print("s\n");

        // CPU info
        print("  CPU: ");
        if max_turbo > 0 {
            print_num(min_eff);
            print("-");
            print_num(max_turbo);
            print(" MHz (HWP auto-scaling)");
        } else {
            print("TSC ");
            print_num(tsc_mhz);
            print(" MHz");
        }
        print("  [");
        print_num(workers);
        print(" workers, ");
        if has_mwait != 0 { print("MWAIT"); } else { print("HLT"); }
        print("]\n");

        print("  ──────────────────────────────────────────────────\n\n");

        // Per-core status with frequency
        print("  CORE    MHz  QUEUE  STATUS\n");
        print("  ────  ─────  ─────  ──────\n");

        let mut i: i64 = 0;
        while i < cores {
            print("  ");
            print_num_padded(i, 4);

            // Per-core MHz
            let mhz = sys(12 | ((i as i32) << 8) as i32);
            print("  ");
            print_num_padded(mhz, 5);

            // Queue length
            let qlen = sys(11 | ((i as i32) << 8) as i32);
            print("  ");
            print_num_padded(qlen, 5);

            // Status
            if i == 0 {
                print("  kernel/irq");
            } else if qlen > 0 {
                print("  working");
            } else {
                print("  idle");
            }
            print("\n");

            i += 1;
        }

        // Memory
        print("\n  MEMORY\n  ──────\n");
        print("  Physical: ");
        print_num(free_mb);
        print(" MB free\n");
        print("  Heap:     ");
        print_num(heap_used / 1024);
        print(" KB / ");
        print_num(heap_total / (1024 * 1024));
        print(" MB  ");
        print_bar(heap_used, heap_total, 20);
        print("\n");

        // Scheduler
        print("\n  SCHEDULER\n  ─────────\n");
        print("  Spawned:   ");
        print_num(spawned);
        print("  Completed: ");
        print_num(completed);
        print("  Steals: ");
        print_num(steals);
        print("\n");

        print("\n  [q] quit\n");

        // Sleep 1 second (renders frame, processes events)
        unsafe { npk_sleep(1000); }

        // Drain input buffer, check for 'q'
        loop {
            let key = unsafe { npk_input_poll() };
            if key < 0 { break; }
            if key == 0x71 || key == 0x51 { return; } // 'q' or 'Q'
        }
    }
}
