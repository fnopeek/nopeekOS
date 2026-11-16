//! Smoke-test /init for the arm64 microVM guest kernel.
//!
//! Mounts devtmpfs, proc and sysfs, prints what the guest sees to
//! /dev/console and powers off through reboot(2), which Linux turns
//! into PSCI SYSTEM_OFF.

use std::fs;
use std::io::Write;

const SYS_MOUNT: usize = 40;
const SYS_MKDIRAT: usize = 34;
const SYS_REBOOT: usize = 142;
const AT_FDCWD: usize = -100isize as usize;

const LINUX_REBOOT_MAGIC1: usize = 0xfee1_dead;
const LINUX_REBOOT_MAGIC2: usize = 0x2812_1969;
const LINUX_REBOOT_CMD_POWER_OFF: usize = 0x4321_fedc;

unsafe fn syscall5(nr: usize, a: usize, b: usize, c: usize, d: usize, e: usize) -> isize {
    let r: isize;
    // SAFETY: the caller passes a valid Linux syscall and arguments that
    // are valid for it; svc clobbers nothing beyond x0.
    unsafe {
        core::arch::asm!("svc 0",
            in("x8") nr,
            inlateout("x0") a => r,
            in("x1") b, in("x2") c, in("x3") d, in("x4") e,
            options(nostack));
    }
    r
}

fn mount(src: &[u8], dst: &[u8], fstype: &[u8]) -> isize {
    // SAFETY: all three are NUL-terminated byte strings that outlive the call.
    unsafe {
        syscall5(SYS_MKDIRAT, AT_FDCWD, dst.as_ptr() as usize, 0o755, 0, 0);
        syscall5(SYS_MOUNT, src.as_ptr() as usize, dst.as_ptr() as usize,
                 fstype.as_ptr() as usize, 0, 0)
    }
}

fn main() {
    let dev = mount(b"devtmpfs\0", b"/dev\0", b"devtmpfs\0");
    let proc = mount(b"proc\0", b"/proc\0", b"proc\0");
    let sys = mount(b"sysfs\0", b"/sys\0", b"sysfs\0");

    let mut con = fs::OpenOptions::new().write(true).open("/dev/console").ok();
    let mut out = |s: &str| {
        if let Some(c) = con.as_mut() {
            let _ = c.write_all(s.as_bytes());
        }
    };

    out("\nhello from the arm64 guest\n");
    out(&format!("mount: dev={dev} proc={proc} sys={sys}\n"));
    out(&format!("uname: {}", fs::read_to_string("/proc/version").unwrap_or_default()));
    let online = fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default();
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    out(&format!("cpus: {cpus} (online {})\n", online.trim()));
    out(&format!("cmdline: {}", fs::read_to_string("/proc/cmdline").unwrap_or_default()));
    out("/proc/interrupts:\n");
    out(&fs::read_to_string("/proc/interrupts").unwrap_or_default());
    out("/sys/bus/pci/devices:\n");
    let mut devs: Vec<String> = fs::read_dir("/sys/bus/pci/devices")
        .map(|d| d.filter_map(|e| e.ok()).map(|e| {
            let p = e.path();
            let id = |f: &str| fs::read_to_string(p.join(f)).unwrap_or_default().trim().to_string();
            format!("  {} {}:{} class {}", e.file_name().to_string_lossy(),
                    id("vendor"), id("device"), id("class"))
        }).collect())
        .unwrap_or_default();
    devs.sort();
    for d in devs {
        out(&d);
        out("\n");
    }
    out("powering off\n");

    // SAFETY: reboot(2) with the documented magic numbers; it does not return
    // on success.
    unsafe {
        syscall5(SYS_REBOOT, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
                 LINUX_REBOOT_CMD_POWER_OFF, 0, 0);
    }
    loop {
        std::thread::park();
    }
}
