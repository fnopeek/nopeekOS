//! top — nopeekOS system monitor (WASM module)
//!
//! Displays per-core status, memory, heap, and scheduler stats.
//! Runs inside the WASM sandbox, reads system info via npk_sys_info().

#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

// --- Host function bindings ---

unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);
    fn npk_sys_info(key: i32) -> i64;
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

fn print_line(label: &str, value: i64, suffix: &str) {
    print("  ");
    print(label);
    print_num(value);
    print(suffix);
    print("\n");
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
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

    print("\n");
    print("  nopeekOS top — ");
    print_num(cores);
    print(" cores (");
    print_num(workers);
    print(" workers, ");
    if has_mwait != 0 { print("MWAIT"); } else { print("HLT"); }
    print(") — up ");
    print_num(uptime / 60);
    print("m ");
    print_num(uptime % 60);
    print("s\n");
    print("  TSC: ");
    print_num(tsc_mhz);
    print(" MHz\n");
    print("  ──────────────────────────────────────────\n");
    print("\n");

    // Per-core status
    print("  CORE  QUEUE  ROLE\n");
    print("  ────  ─────  ────\n");
    let mut i: i64 = 0;
    while i < cores {
        print("  ");
        if i < 10 { print(" "); }
        print_num(i);
        print("    ");
        let qlen = sys(11 | ((i as i32) << 8) as i32);
        if qlen < 10 { print(" "); }
        if qlen < 100 { print(" "); }
        print_num(qlen);
        if i == 0 {
            print("    kernel/irq\n");
        } else {
            if qlen > 0 {
                print("    worker (busy)\n");
            } else {
                print("    worker (idle)\n");
            }
        }
        i += 1;
    }

    print("\n");
    print("  MEMORY\n");
    print("  ──────\n");
    print_line("Physical:   ", free_mb, " MB free");
    print("  Heap:       ");
    print_num(heap_used / 1024);
    print(" KB / ");
    print_num(heap_total / (1024 * 1024));
    print(" MB\n");

    print("\n");
    print("  SCHEDULER\n");
    print("  ─────────\n");
    print_line("Spawned:    ", spawned, "");
    print_line("Completed:  ", completed, "");
    print_line("Steals:     ", steals, "");
    print("\n");
}
