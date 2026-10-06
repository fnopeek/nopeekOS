//! i2c_hid.wasm — touchpads and digitizers that are not on PCI.
//!
//! Loads the DSDT, finds every HID-over-I2C device, logs controller,
//! address, bus speed, descriptor register and GPIO pin, then brings up the
//! bus and the device and feeds pointer motion and gestures to the kernel.
//!
//! The ACPI part lives in [`i2c_hid_core`] and is tested on the host
//! against a real firmware table (`hp_dsdt_finds_the_touchpad`).
//!
//! Reports are read only when the interrupt pin says one is pending
//! ([`Gate`]); a blind read costs a full bus transfer per attempt.

#![no_std]

extern crate alloc;

use aml_core::{Ec, Machine, Namespace};
use i2c_hid_core::report;

// Raw access to firmware and hardware: HARDWARE (bit 0x40). A `.npk.caps`
// section replaces the default and must list READ itself to keep it; none
// is needed here.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x40];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    logln("[i2c-hid] panic");
    // Trap, do not spin: the kernel catches it and reports it on screen.
    // `loop {}` would be a silent hang.
    core::arch::wasm32::unreachable()
}

use core::sync::atomic::{AtomicBool, Ordering};
use npk_sys::bump::Bump;
use npk_sys::cell::Single;

// ── Diagnostic lines: built in, silent in normal operation ───────────
//
// Raw descriptor, first reports, periodic statistics. Queried once at start
// (`npk_sys_info(50)` = config value `log.drivers`); afterwards each check
// costs one comparison.
//
// Not gated here: what the driver decides. Which device was found, whether
// precision mode took effect, the state of the gate and every error are
// always logged.
static VERBOSE: AtomicBool = AtomicBool::new(false);

fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Like `logln`, but only when `set log.drivers 1` is set.
fn dbgln(s: &str) {
    if verbose() { logln(s); }
}

/// Hardware access for the bus driver.
///
/// The driver computes, this wrapper accesses — so the same code runs in
/// the harness against a mock.
struct HostBus { handle: i32 }

impl i2c_hid_core::dw_i2c::Bus for HostBus {
    fn read32(&mut self, off: u32) -> u32 {
        npk_sys::mmio_read32(self.handle, off as i32) as u32
    }
    fn write32(&mut self, off: u32, val: u32) {
        npk_sys::mmio_write32(self.handle, off as i32, val as i32);
    }
    fn now_us(&mut self) -> u64 {
        let t = npk_sys::now_us();
        if t < 0 { 0 } else { t as u64 }
    }
    fn udelay(&mut self, us: u32) {
        // From one millisecond on, yield instead of spinning.
        //
        // wasmi meters fuel per WASM instruction, and a busy-wait against
        // the clock burns through the module's budget in seconds.
        // `npk_sleep` yields to the scheduler and costs one instruction.
        //
        // Below that we keep spinning: `npk_sleep` works in milliseconds,
        // and an I2C cycle takes 2.5 us.
        if us >= 1000 {
            npk_sys::sleep((us / 1000) as i32);
            return;
        }
        let end = self.now_us() + us as u64;
        while self.now_us() < end { core::hint::spin_loop(); }
    }
    fn note(&mut self, s: &str) { logln(s); }
}

fn log(s: &str) {
    npk_sys::print(s.as_bytes());
}
fn logln(s: &str) { log(s); log("\n"); }

// ── Bump allocator: the run is one-shot ──────────────────────────────
const HEAP_SIZE: usize = 16 * 1024 * 1024;
#[global_allocator]
static ALLOC: Bump<HEAP_SIZE> = Bump::new();

/// "SSDT" as the four characters appear in memory (little-endian).
const SIG_SSDT: [u8; 4] = *b"SSDT";
/// Besides SSDT, ACPICA also loads PSDT and OSDT into the namespace
/// (`acpi_tb_load_namespace`). Rare, but it costs nothing.
const SIG_PSDT: [u8; 4] = *b"PSDT";
const SIG_OSDT: [u8; 4] = *b"OSDT";

const DSDT_MAX: usize = 512 * 1024;
static DSDT: Single<[u8; DSDT_MAX]> = Single::new([0; DSDT_MAX]);
/// Room for one SSDT at a time. The namespace copies out what it needs, so
/// the buffer may be reused afterwards.
const SSDT_MAX: usize = 256 * 1024;
static SSDT: Single<[u8; SSDT_MAX]> = Single::new([0; SSDT_MAX]);

/// Firmware access for the interpreter.
///
/// An I2C HID device needs no embedded controller, but it does need
/// SystemMemory: the I2C controllers' `_STA` may read a configuration byte
/// from the firmware's NVS window. Without this access the interpreter
/// would return 0 there, and the firmware would conclude the controllers
/// are disabled.
///
/// `npk_acpi_mem_read` is read-only and rejects any address in the RAM map.
struct FirmwareAccess;
impl Ec for FirmwareAccess {
    fn read(&mut self, _a: u8) -> u8 { 0 }
    fn write(&mut self, _a: u8, _v: u8) {}
    fn mem_read(&mut self, addr: u64) -> Option<u8> {
        let v = npk_sys::acpi_mem_read((addr >> 32) as i32, addr as u32 as i32);
        if v < 0 { None } else { Some(v as u8) }
    }
    fn note(&mut self, s: &str) { dbgln(s); }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    VERBOSE.store(npk_sys::sys_info(50) == 1, Ordering::Relaxed);
    logln("[i2c-hid] looking for a HID-over-I2C device in the firmware tables");

    let dsdt = DSDT.with(|d| {
        let len = npk_sys::acpi_dsdt(d);
        if len <= 0 || len as usize > DSDT_MAX { return None; }
        // The kernel wrote exactly `len` bytes.
        Some(Namespace::load(&d[..len as usize]))
    });
    let Some(dsdt) = dsdt else {
        logln("[i2c-hid] no DSDT, or bigger than our buffer — nothing to do");
        return;
    };

    let mut ns = match dsdt {
        Ok(ns) => ns,
        Err(_) => { logln("[i2c-hid] DSDT did not parse"); return; }
    };

    // Plus every SSDT.
    //
    // Firmware spreads its declarations over the DSDT and any number of
    // SSDTs; together they form one namespace, and Linux loads them all.
    // Reading only the DSDT misses names defined elsewhere, e.g. the base
    // of the region holding the I2C controllers' enable bits.
    let mut loaded = 0u32;
    for (sig, name) in [(SIG_SSDT, "SSDT"), (SIG_PSDT, "PSDT"), (SIG_OSDT, "OSDT")] {
    for i in 0..32 {
        let (n, r) = SSDT.with(|t| {
            let n = npk_sys::acpi_table(sig, i, t);
            // The kernel wrote exactly `n` bytes.
            let r = (n > 0 && n as usize <= SSDT_MAX).then(|| ns.load_more(&t[..n as usize]));
            (n, r)
        });
        if n <= 0 { break; }
        let _ = name;
        let Some(r) = r else {
            logln(&alloc::format!("[i2c-hid] SSDT {i} is {n} bytes — bigger than our buffer"));
            continue;
        };
        match r {
            Ok(()) => loaded += 1,
            Err(e) => logln(&alloc::format!("[i2c-hid] {name} {i} did not parse: {e}")),
        }
    }
    }
    logln(&alloc::format!("[i2c-hid] namespace: DSDT + {loaded} more table(s)"));

    let mut ec = FirmwareAccess;
    // Resolve conditional declarations at scope level: ACPICA executes the
    // term list while loading, so an `If` there is a real branch. Some
    // firmware defines the base of the I2C enable-bit region this way.
    let (seen, taken) = ns.resolve_conditionals(&mut ec);
    logln(&alloc::format!(
        "[i2c-hid] scope-level conditionals: {seen} seen, {taken} taken"));

    let mut m = Machine::new(&ns, &mut ec);
    m.init();

    let found = i2c_hid_core::discover::find(&ns, &mut m);
    // If a `_STA` returns zero, evaluate it again with tracing to show where
    // the zero came from. Controllers only, and only then — the trace is
    // noisy.
    for d in &found {
        if let Some(c) = &d.controller {
            if !c.present {
                dbgln(&alloc::format!(
                    "[i2c-hid] why is {} absent? tracing its _STA:",
                    aml_core::path_str(&c.path)));
                let mut p = c.path.clone();
                p.push(aml_core::seg("_STA"));
                match m.call_traced(&p, alloc::vec::Vec::new()) {
                    Ok(v) => dbgln(&alloc::format!("[i2c-hid]   _STA returned {:#x}", v.as_int())),
                    Err(e) => dbgln(&alloc::format!("[i2c-hid]   _STA failed: {e}")),
                }
            }
        }
    }
    if found.is_empty() {
        // This is an answer, not a failure: a machine without a touchpad
        // says exactly this.
        logln("[i2c-hid] no HID-over-I2C device declared — idle");
        return;
    }
    let mut live: alloc::vec::Vec<Live> = alloc::vec::Vec::new();
    for d in &found {
        for line in i2c_hid_core::discover::report(d) {
            logln(&line);
        }
        if let Some(l) = probe_bus(d) { live.push(l); }
    }
    if live.is_empty() {
        logln("[i2c-hid] no pointer device came up — idle");
        return;
    }
    logln(&alloc::format!("[i2c-hid] {} pointer device(s) live", live.len()));
    let mut irq = arm_irq(&found, &live);

    // Steady state. A driver does not return; it listens.
    //
    // 5 ms interval: a touchpad reports at about 100-200 Hz, and
    // `npk_sleep` yields to the scheduler in between, costing neither CPU
    // nor fuel.
    let mut buf = [0u8; 64];
    // Three minutes of statistics, then quiet.
    let mut stat_lines_left = 18u32;
    let mut next_stat_us = { let t = npk_sys::now_us(); if t < 0 { 0 } else { t as u64 } }
        + 10_000_000;
    loop {
        // Did the switch to precision mode take effect?
        //
        // Not decided by time: an untouched touchpad says nothing, so a
        // timeout would revert the mode before the first finger lands.
        // The answerable question is: do reports arrive, but never the
        // touchpad report? Then the switch did not take. Silence proves
        // nothing and must not trigger anything.
        for l in live.iter_mut() {
            if l.touch_rid.is_some() && !l.saw_touch && l.other_seen >= 64 {
                if let Some(rid) = l.switched.take() {
                    logln(&alloc::format!(
                        "[i2c-hid] {:#04x}: 64 reports and none is the touchpad one — \
                         the mode switch did not take, back to mouse mode", l.addr));
                    let (dw, desc, addr) = (
                        i2c_hid_core::dw_i2c::Dw { ..l.dw }, l.desc, l.addr);
                    let zero = alloc::vec![0u8; l.mode_len];
                    let _ = i2c_hid_core::hid::set_report(
                        &mut l.bus, &dw, addr, &desc,
                        i2c_hid_core::hid::REPORT_TYPE_FEATURE, rid, &zero);
                }
            }
        }
        let mut alive = false;
        for l in live.iter_mut() {
            // Ask the pin first, then touch the bus.
            //
            // A read fetches `wMaxInputLength` bytes, up to 64, i.e. about
            // 1.4 ms on the bus at 400 kHz. The pin costs one register read.
            let (poll_now, gate_said_no) = gate_check(l);
            if !poll_now {
                // A device that was not asked cannot be silent; the last
                // real result stands.
                l.skips += 1;
                if !l.dead { alive = true; }
                continue;
            }
            let mut got = false;
            let mut answered = false;
            // Drain the line, not one report per round.
            //
            // A frame of two reports would otherwise take two rounds, which
            // at a 5 ms interval matches the device's report rate, so a
            // report is lost whenever the device is faster. Eight is the cap
            // so a chatty device cannot monopolise the round.
            for i in 0..8 {
                l.polls += 1;
                match poll_live(l, &mut buf) {
                    Step::Data => {
                        answered = true;
                        got = true;
                        l.datas += 1;
                        if i == 7 { l.capped += 1; }
                        // The level stays asserted until the report is
                        // read; once it drops nothing is pending. An empty
                        // extra read would cost a whole transfer.
                        if !gate_asserted_now(l) { break; }
                    }
                    Step::Empty => { answered = true; l.empties += 1; break; }
                    Step::Junk => { answered = true; l.junk += 1; break; }
                    Step::Dead => { l.errs += 1; break; }
                }
            }
            l.dead = !answered;
            if answered { alive = true; }
            gate_verdict(l, gate_said_no, got);
        }
        if !alive {
            logln("[i2c-hid] all devices stopped answering — giving up");
            return;
        }

        // Every ten seconds, log what the round actually cost, then stop
        // on its own.
        let now = { let t = npk_sys::now_us(); if t < 0 { 0 } else { t as u64 } };

        // A tap presses immediately and releases later, or it could never
        // become a drag. The tick lives here and not in the report path: an
        // untouched touchpad sends no report, and the button would stay
        // down.
        for l in live.iter_mut() {
            l.track.tick(now / 1000);
            push_buttons(l);
        }

        if stat_lines_left > 0 && now >= next_stat_us {
            next_stat_us = now + 10_000_000;
            stat_lines_left -= 1;
            for l in live.iter_mut() {
                dbgln(&alloc::format!(
                    "[i2c-hid] {:#04x}: 10 s — {} read(s): {} data, {} empty, \
                     {} undeclared, {} FAILED · {} rounds skipped by the pin · \
                     {} drain caps",
                    l.addr, l.polls, l.datas, l.empties, l.junk, l.errs,
                    l.skips, l.capped));
                l.polls = 0; l.datas = 0; l.empties = 0; l.junk = 0;
                l.errs = 0; l.skips = 0; l.capped = 0;
            }
        }
        match &mut irq {
            Some(q) => {
                // `do_amd_gpio_irq_handler`: every pending pin acknowledged
                // (writing back what was read clears its status bits), then
                // the EOI to the GPIO unit. The level line the kernel masked
                // is released when we wait again.
                use i2c_hid_core::gpio;
                // All pending pins of the block, as `do_amd_gpio_irq_handler`
                // does — the line is shared by every pin. A pin the firmware
                // enabled (lid, hotkeys, EC) with its status set would keep
                // the level line asserted forever. Ours are acknowledged;
                // any other pending pin is not an interrupt anybody here
                // handles, so it is masked — Linux: "Disabling spurious GPIO
                // IRQ".
                let rd = |o: u32| npk_sys::mmio_read32(q.handle, o as i32) as u32;
                let wr = |o: u32, v: u32| npk_sys::mmio_write32(q.handle, o as i32, v as i32);
                let status = ((rd(q.block_off + gpio::WAKE_INT_STATUS_REG1) as u64) << 32
                    | rd(q.block_off + gpio::WAKE_INT_STATUS_REG0) as u64)
                    & ((1u64 << 46) - 1);
                for bit in 0..46u32 {
                    if status & (1u64 << bit) == 0 { continue; }
                    for i in 0..4u32 {
                        let off = q.block_off + (bit * 4 + i) * 4;
                        let v = rd(off);
                        if v & gpio::PIN_IRQ_PENDING == 0 || v & gpio::INTERRUPT_MASK == 0 {
                            continue;
                        }
                        if q.pins.contains(&off) {
                            wr(off, v);
                        } else {
                            wr(off, v & !gpio::INTERRUPT_MASK);
                            if q.spurious_logged < 8 {
                                logln(&alloc::format!(
                                    "[i2c-hid] interrupt: GPIO pin {} pending but nobody's — masked",
                                    bit * 4 + i));
                            }
                            q.spurious_logged += 1;
                        }
                    }
                }
                let mr = q.block_off + gpio::WAKE_INT_MASTER_REG;
                let m = npk_sys::mmio_read32(q.handle, mr as i32) as u32;
                npk_sys::mmio_write32(q.handle, mr as i32, (m | gpio::EOI_MASK) as i32);

                // Sleep until the pad reports. Wake early only for our own
                // timers: an open tap releases its button after TAP_MS, and
                // the initial statistics period. At most one second, so a
                // lost interrupt shows as lag rather than a dead pointer.
                let now = { let t = npk_sys::now_us(); if t < 0 { 0 } else { t as u64 } };
                let now_ms = now / 1000;
                let mut wait_ms: u64 = 1000;
                for l in live.iter() {
                    if let Some(d) = l.track.next_deadline() {
                        wait_ms = wait_ms.min(d.saturating_sub(now_ms).max(1));
                    }
                }
                if stat_lines_left > 0 {
                    wait_ms = wait_ms.min((next_stat_us.saturating_sub(now) / 1000).max(1));
                }
                const WAIT_IRQ: i32 = 2;
                npk_sys::wait(WAIT_IRQ, wait_ms as i32);
            }
            None => { let _ = npk_sys::sleep(5); }
        }
    }
}

/// The GPIO controller's interrupt, set up for every live pad.
struct IrqMode {
    handle: i32,
    /// Offset of the GPIO block inside the mapped page.
    block_off: u32,
    /// Each pad's pin register (page offset).
    pins: alloc::vec::Vec<u32>,
    /// How many foreign pending pins were masked (first few are logged).
    spurious_logged: u32,
}

/// Wait on the GPIO controller's interrupt instead of polling, or say why
/// not. Needs every live pad gated on a pin of the same AMD block, and that
/// block's own line in its `_CRS`. Pin setup as `amd_gpio_irq_set_type`
/// (level, polarity, clear status, the enable-and-wait-for-debounce dance)
/// followed by `amd_gpio_irq_enable` (enable + unmask).
fn arm_irq(found: &[i2c_hid_core::discover::HidDevice], live: &[Live]) -> Option<IrqMode> {
    use i2c_hid_core::gpio;
    let mut handle = -1;
    let mut pins = alloc::vec::Vec::new();
    let mut lows = alloc::vec::Vec::new();
    for l in live {
        let Gate::Pin { handle: h, reg_off, active_low, .. } = &l.gate else {
            logln("[i2c-hid] interrupt: a pad reads blind — staying on the 5 ms poll");
            return None;
        };
        if handle >= 0 && *h != handle {
            logln("[i2c-hid] interrupt: pads on different GPIO blocks — staying on the 5 ms poll");
            return None;
        }
        handle = *h;
        pins.push(*reg_off);
        lows.push(*active_low);
    }
    let g = found.iter().find_map(|d| d.gpio_controller.as_ref())?;
    let Some((gsi, flags)) = g.irq else {
        logln("[i2c-hid] interrupt: the GPIO block names no line in its _CRS — staying on the 5 ms poll");
        return None;
    };
    let level = flags & 0x02 == 0;
    let low = flags & 0x04 != 0;
    let v = npk_sys::irq_register_gsi(gsi as i32, (level as i32) | ((low as i32) << 1));
    if v < 0 {
        logln(&alloc::format!(
            "[i2c-hid] interrupt: GSI {gsi} refused (taken, or no I/O APIC) — staying on the 5 ms poll"));
        return None;
    }
    for (&off, &active_low) in pins.iter().zip(lows.iter()) {
        let o = off as i32;
        let cfg = gpio::irq_level_config(npk_sys::mmio_read32(handle, o) as u32, active_low);
        // Enable while still masked, wait for the enable bit to read back
        // (the debounce settles), then write the plain configuration.
        npk_sys::mmio_write32(handle, o, ((cfg | gpio::INTERRUPT_ENABLE) & !gpio::INTERRUPT_MASK) as i32);
        for _ in 0..100_000 {
            if npk_sys::mmio_read32(handle, o) as u32 & gpio::INTERRUPT_ENABLE != 0 { break; }
        }
        npk_sys::mmio_write32(handle, o, cfg as i32);
        // `amd_gpio_irq_enable`.
        let r = npk_sys::mmio_read32(handle, o) as u32;
        npk_sys::mmio_write32(handle, o, (r | gpio::INTERRUPT_ENABLE | gpio::INTERRUPT_MASK) as i32);
    }
    let block_off = g.mmio_base & 0xFFF;
    let mr = (block_off + gpio::WAKE_INT_MASTER_REG) as i32;
    let m = npk_sys::mmio_read32(handle, mr) as u32;
    npk_sys::mmio_write32(handle, mr, (m | gpio::EOI_MASK) as i32);
    logln(&alloc::format!(
        "[i2c-hid] interrupt: GSI {gsi} ({}, active-{}) on vector {v} — the pads wake the driver",
        if level { "level" } else { "edge" }, if low { "low" } else { "high" }));
    Some(IrqMode { handle, block_off, pins, spurious_logged: 0 })
}

/// Touch the controller: map it, read its ID, compute the counts.
///
/// This is the first step that touches hardware, and the component type is
/// the cheapest proof that mapping and address are right. If it reads
/// `0x44570140` ("DW" + 0x0140), the whole path so far is correct: DSDT
/// read, `_CRS` evaluated, MMIO mapped.
fn probe_bus(d: &i2c_hid_core::discover::HidDevice) -> Option<Live> {
    use i2c_hid_core::dw_i2c;

    let c = match &d.controller {
        Some(c) if c.mmio_base != 0 => c,
        _ => { logln("[i2c-hid]   controller has no fixed MMIO — nothing to map"); return None; }
    };

    // `_STA` only warns here; it does not block.
    //
    // `_STA` is evaluated by an interpreter with known gaps: operation
    // regions without backing memory yield 0, so a zero may be an artefact
    // and should not outrank a direct hardware read.
    //
    // The line is drawn between reading and writing: reading the ID is
    // always allowed (a read in the FCH range at worst returns all ones);
    // configuration only happens once it matches.
    if !c.present {
        logln("[i2c-hid]   _STA says absent — reading the signature anyway, \
               writes only if it checks out");
    }

    let pages = ((c.mmio_len as usize).max(4096) + 4095) / 4096;
    let handle = npk_sys::mmio_map_phys(0, c.mmio_base as i32, pages.min(16) as i32);
    if handle < 0 {
        logln("[i2c-hid]   MMIO mapping REFUSED — no HARDWARE right, or the range is RAM");
        return None;
    }

    let mut bus = HostBus { handle };
    let clk_khz = c.input_clock_hz / 1000;
    match dw_i2c::Dw::setup(&mut bus, d.bus_speed_hz, clk_khz, c.sscn, c.fmcn) {
        Ok(dw) => {
            dbgln(&alloc::format!("[i2c-hid]   {}", dw.describe()));
            dbgln("[i2c-hid]   Designware signature OK — the controller is really there");
            let mut live = talk_to_device(&mut bus, &dw, d)?;
            live.gate = arm_gate(d);
            return Some(live);
        }
        Err(dw_i2c::Error::NotDesignware(v)) => {
            logln(&alloc::format!(
                "[i2c-hid]   COMP_TYPE {v:#010x}, expected 0x44570140 — wrong address,  or the block is powered down"));
        }
        Err(e) => logln(&alloc::format!("[i2c-hid]   controller setup failed: {e:?}")),
    }
    None
}

/// How a device reports its pointer data.
enum Mode {
    /// Mouse emulation: one X, one Y, buttons, maybe a wheel.
    Mouse {
        fx: report::Field,
        fy: report::Field,
        wheel: Option<report::Field>,
    },
    /// Precision touchpad: contact points. Per finger a tip switch, an X and
    /// a Y; gestures, which no device reports, are derived from them.
    ///
    /// The `Contact Identifier` is carried along: with two fingers down, the
    /// motion must be computed from the same finger. Without it the
    /// reference is "first contact in the frame", and when the device
    /// reorders contacts the distance jumps by the finger spacing.
    Touchpad {
        contacts: alloc::vec::Vec<Contact>,
        count: Option<report::Field>,
    },
}

/// A contact slot in the report: is it touching, which finger, where.
struct Contact {
    tip: report::Field,
    id: Option<report::Field>,
    x: report::Field,
    y: report::Field,
}

/// A report and how to read it.
struct Decoder {
    rid: u8,
    mode: Mode,
    btn: alloc::vec::Vec<report::Field>,
}

/// A configured device from which pointer motion can be read.
struct Live {
    bus: HostBus,
    dw: i2c_hid_core::dw_i2c::Dw,
    addr: u16,
    desc: i2c_hid_core::hid::HidDesc,
    uses_ids: bool,
    /// All reports we can read, not just one.
    ///
    /// If the switch to precision mode does not take, the device keeps
    /// sending its mouse report. Like Linux, incoming reports are
    /// dispatched to a decoder by their report ID instead of expecting a
    /// single ID.
    decoders: alloc::vec::Vec<Decoder>,
    /// Log the first few unknown report IDs.
    unknown_logged: u32,
    /// Did we switch to precision mode, and which feature report holds the
    /// switch?
    switched: Option<u8>,
    /// Length of that feature report. Reverting must use the same length as
    /// setting, or it is discarded too.
    mode_len: usize,
    /// How many reports have arrived so far?
    seen: u32,
    /// ID of the touchpad report, if there is one.
    touch_rid: Option<u8>,
    /// Has it ever arrived? That is the proof the switch took effect.
    saw_touch: bool,
    /// How many reports arrived that are not the touchpad report?
    other_seen: u32,
    /// How many contact states have been logged? The first few go to the
    /// log; otherwise nothing shows whether two fingers arrive.
    touch_logged: u32,
    /// The first reports raw. What the device really sends is not visible
    /// in any derived number.
    raw_logged: u32,
    /// The first scroll decisions.
    scroll_logged: u32,
    /// The first taps.
    tap_logged: u32,

    // ── Positions become motion and gestures ─────────────────────
    //
    // That logic lives in `i2c_hid_core::gesture`, where it has tests.
    track: i2c_hid_core::gesture::Tracker,
    /// Reference point for a mouse that reports positions instead of motion.
    have_ref: bool,
    rx: i32,
    ry: i32,
    /// The last reported button state.
    ///
    /// A release is an event just like a press: feeding only when
    /// `buttons != 0` reports the press and never the end, and the
    /// compositor considers the button held forever.
    last_buttons: i32,
    /// The physical buttons from the last report, excluding what a tap is
    /// currently holding.
    ///
    /// They are kept apart because they arrive at different times: the
    /// physical state comes with a report, the held one expires on a timer
    /// that ticks even when the device is silent.
    hw_buttons: i32,

    /// Do we ask the pin before touching the bus?
    gate: Gate,
    // ── What the last ten seconds cost ───────────────────────────
    //
    // Periodic cost statistics; they stop on their own.
    polls: u32,
    datas: u32,
    empties: u32,
    junk: u32,
    errs: u32,
    skips: u32,
    capped: u32,
    /// The first few failures with their reason. A silent failure is the
    /// most expensive state: xfer waits up to one second.
    err_logged: u32,
    /// Was this device silent on the last real read attempt?
    ///
    /// A skipped round is not silence; nothing was asked. Without this flag
    /// the gate would defeat the emergency brake: a dead bus would look like
    /// a resting touchpad.
    dead: bool,
}

/// The pin that says whether a report is pending at all.
///
/// A read is not cheap: it fetches `wMaxInputLength` bytes (up to 64 here,
/// capped by the buffer), about 1.4 ms on the bus at 400 kHz, two hundred
/// times a second per device.
///
/// Linux never reads blind: `i2c_hid_get_input` is called only from
/// `i2c_hid_irq`. Without the interrupt, the level stays asserted until the
/// report is read, so it can be polled for the cost of one register read.
enum Gate {
    /// No pin, no known block, or it proved wrong: read as before.
    Blind,
    /// The pin is set up and queried.
    Pin {
        handle: i32,
        reg_off: u32,
        active_low: bool,
        /// How many consecutive rounds did it say "nothing pending"?
        skipped: u32,
        /// How often did a report arrive anyway while it said "nothing"?
        contradictions: u32,
        /// Has it ever correctly announced a report? Logged once.
        proved: bool,
    },
}

/// Cross-check: after this many skipped rounds, read anyway.
///
/// Silence proves nothing — a resting touchpad says nothing, and a pin that
/// always reports "nothing" looks the same. What proves something is the
/// reverse: a report arriving although the pin said no. So every 100 ms a
/// blind read is made, and three such contradictions in a row disable the
/// gate permanently.
const GATE_CROSS_CHECK_ROUNDS: u32 = 20;
/// Once the pin has correctly announced a report it is trusted; one
/// heartbeat per second suffices and the cross-check costs nothing more.
const GATE_CROSS_CHECK_PROVED: u32 = 200;
const GATE_MAX_CONTRADICTIONS: u32 = 3;

/// Talk to the device: set up the bus, probe the address, fetch the HID
/// descriptor, power on and reset.
///
/// From here on we write. The justification is the register value the
/// controller just returned itself, not a firmware flag.
fn talk_to_device(
    bus: &mut HostBus,
    dw: &i2c_hid_core::dw_i2c::Dw,
    d: &i2c_hid_core::discover::HidDevice,
) -> Option<Live> {
    use i2c_hid_core::{dw_i2c, hid, report};

    let addr = d.slave_address;
    let desc_reg = match d.descriptor_address {
        Some(r) => r,
        None => { logln("[i2c-hid]   no descriptor register — cannot talk to it"); return None; }
    };

    dw_i2c::init_master(bus, dw);
    dbgln("[i2c-hid]   master initialised");

    match hid::probe_address(bus, dw, addr) {
        Ok(()) => dbgln(&alloc::format!("[i2c-hid]   device at {addr:#04x} answers")),
        Err(e) => {
            logln(&alloc::format!("[i2c-hid]   device at {addr:#04x} does not answer: {e:?}"));
            return None;
        }
    }

    let desc = match hid::fetch_descriptor(bus, dw, addr, desc_reg) {
        Ok(x) => x,
        Err(e) => { logln(&alloc::format!("[i2c-hid]   {e}")); return None; }
    };
    logln(&alloc::format!("[i2c-hid]   {}", desc.describe()));

    match hid::reset(bus, dw, addr, &desc) {
        Ok(()) => dbgln("[i2c-hid]   power on + reset done"),
        Err(e) => { logln(&alloc::format!("[i2c-hid]   {e}")); return None; }
    }


    // Fetch and parse the report descriptor. What a report byte means is
    // stated there and nowhere else; without it a driver fits exactly one
    // model.
    let n = desc.report_desc_length as usize;
    if n == 0 || n > 4096 {
        logln("[i2c-hid]   no usable report descriptor length");
        return None;
    }
    let mut rd = alloc::vec![0u8; n];
    if let Err(e) = hid::read_register(bus, dw, addr, desc.report_desc_register, &mut rd) {
        logln(&alloc::format!("[i2c-hid]   report descriptor read failed: {e:?}"));
        return None;
    }
    let map = report::parse(&rd);
    dbgln(&alloc::format!("[i2c-hid]   {}", map.describe()));

    // Log the raw descriptor if it is small enough.
    //
    // The bytes are the ground truth, not our interpretation of them (e.g.
    // how many contact slots the device declares). Large descriptors are
    // left out.
    if n <= 512 {
        dbgln(&alloc::format!("[i2c-hid]   raw report descriptor, {n} bytes:"));
        for (i, chunk) in rd.chunks(24).enumerate() {
            dbgln(&alloc::format!("[i2c-hid]   rd {:03x} {:02x?}", i * 24, chunk));
        }
    }

    // If the device has a "Device Mode", set it to 3.
    //
    // A precision touchpad starts in mouse emulation: one X, one Y, buttons
    // and no contact points, so the second finger is never reported. The
    // switch is in a feature report (Digitizer 0x52).
    let mut switched: Option<u8> = None;
    let mut mode_len: usize = 1;
    if let Some(im) = map.find_feature(report::PAGE_DIGITIZER, report::USAGE_INPUT_MODE) {
        let im = *im;
        // The length comes from the descriptor.
        //
        // Some touchpads declare `Input Mode` with `Report Size 16`, a
        // two-byte feature report. A shorter one is ACKed on the bus but
        // discarded: no error, no effect. Padding bits count, which is why
        // `report_bytes` is used and not the sum of the fields.
        let n = map.report_bytes(report::Kind::Feature, im.report_id).max(1);
        mode_len = n;
        let mut payload = alloc::vec![0u8; n];
        report::insert(&mut payload, &im, 3);
        match hid::set_report(bus, dw, addr, &desc, hid::REPORT_TYPE_FEATURE, im.report_id, &payload) {
            Ok(()) => {
                logln(&alloc::format!(
                    "[i2c-hid]   device mode -> 3 (precision touchpad), feature report {} \
                     ({n} byte(s): {payload:02x?})",
                    im.report_id));
                switched = Some(im.report_id);
            }
            Err(e) => logln(&alloc::format!(
                "[i2c-hid]   device mode switch failed: {e:?} — staying in mouse mode")),
        }
    }

    // Set up every report we can read, touchpad and mouse. The device
    // decides which one comes.
    let mut decoders: alloc::vec::Vec<Decoder> = alloc::vec::Vec::new();
    let mut scroll_step = 1i32;
    let mut hscroll_step = 1i32;
    let mut tap_move = 1i32;
    let mut pin_move = 1i32;

    if let Some(id) = map.touchpad_report() {
        let tips = map.find_all(report::Kind::Input, id, report::PAGE_DIGITIZER, report::USAGE_TIP_SWITCH);
        let xs = map.find_all(report::Kind::Input, id, report::PAGE_GENERIC_DESKTOP, report::USAGE_X);
        let ys = map.find_all(report::Kind::Input, id, report::PAGE_GENERIC_DESKTOP, report::USAGE_Y);
        let ids = map.find_all(report::Kind::Input, id, report::PAGE_DIGITIZER, report::USAGE_CONTACT_ID);
        let n = tips.len().min(xs.len()).min(ys.len());
        if n > 0 {
            let contacts: alloc::vec::Vec<_> = (0..n)
                .map(|i| Contact {
                    tip: *tips[i],
                    id: ids.get(i).map(|f| **f),
                    x: *xs[i],
                    y: *ys[i],
                })
                .collect();
            // Scroll step from the logical range: about a fortieth of the
            // pad height per detent. Device-independent, since the number
            // comes from the device itself.
            let span = (ys[0].logical_max - ys[0].logical_min).max(1);
            let step = (span / 40).max(1);
            // How far a finger may move and still count as a tap: about an
            // eightieth of the pad width, roughly 1.3 mm, the same value
            // libinput uses. Derived from the device, not guessed in pixels.
            tap_move = ((xs[0].logical_max - xs[0].logical_min).max(1) / 80).max(1);
            // How far it may move under a pressed button before the pointer
            // follows again: twice that, roughly 2.6 mm. Pressing deforms
            // the fingertip, and its wandering centroid is not pointer
            // motion.
            pin_move = (tap_move * 2).max(1);
            // The horizontal detent comes from the width, not the height:
            // the pad is wider than tall, and a step from the height would
            // be too fine horizontally.
            hscroll_step = (((xs[0].logical_max - xs[0].logical_min).max(1)) / 40).max(1);
            logln(&alloc::format!(
                "[i2c-hid]   report {id}: touchpad, {n} contact slot(s), \
                 scroll step {step}/{hscroll_step}, tap move {tap_move}, \
                 pin move {pin_move}, ids {}",
                if ids.is_empty() { "no" } else { "yes" }));
            scroll_step = step;
            decoders.push(Decoder {
                rid: id,
                mode: Mode::Touchpad {
                    contacts,
                    count: map.find(id, report::PAGE_DIGITIZER, report::USAGE_CONTACT_COUNT).copied(),
                },
                btn: (1u16..=3).filter_map(|u| map.find(id, report::PAGE_BUTTON, u).copied()).collect(),
            });
        }
    }

    for id in map.report_ids() {
        if decoders.iter().any(|d| d.rid == id) { continue; }
        let (Some(fx), Some(fy)) = (
            map.find(id, report::PAGE_GENERIC_DESKTOP, report::USAGE_X),
            map.find(id, report::PAGE_GENERIC_DESKTOP, report::USAGE_Y),
        ) else { continue };
        let (fx, fy) = (*fx, *fy);
        let wheel = map.find(id, report::PAGE_GENERIC_DESKTOP, report::USAGE_WHEEL).copied();
        logln(&alloc::format!(
            "[i2c-hid]   report {id}: mouse ({}), wheel {}",
            if fx.relative { "relative" } else { "absolute" },
            if wheel.is_some() { "yes" } else { "no" }));
        decoders.push(Decoder {
            rid: id,
            mode: Mode::Mouse { fx, fy, wheel },
            btn: (1u16..=3).filter_map(|u| map.find(id, report::PAGE_BUTTON, u).copied()).collect(),
        });
    }

    if decoders.is_empty() {
        logln("[i2c-hid]   no report carries X and Y — not a pointer");
        return None;
    }
    logln(&alloc::format!("[i2c-hid]   {} decoder(s), ready", decoders.len()));

    Some(Live {
        bus: HostBus { handle: bus.handle },
        dw: i2c_hid_core::dw_i2c::Dw { ..*dw },
        addr, desc,
        uses_ids: map.uses_ids,
        touch_rid: decoders.iter()
            .find(|d| matches!(d.mode, Mode::Touchpad { .. }))
            .map(|d| d.rid),
        decoders,
        unknown_logged: 0,
        switched,
        mode_len,
        seen: 0,
        saw_touch: false,
        other_seen: 0,
        touch_logged: 0,
        raw_logged: 0, scroll_logged: 0, tap_logged: 0,
        track: i2c_hid_core::gesture::Tracker::new(scroll_step, hscroll_step, tap_move, pin_move),
        have_ref: false, rx: 0, ry: 0,
        last_buttons: 0, hw_buttons: 0,
        gate: Gate::Blind,
        dead: false,
        polls: 0, datas: 0, empties: 0, junk: 0, errs: 0, skips: 0, capped: 0,
        err_logged: 0,
    })
}

/// Outcome of a single read attempt.
enum Step {
    /// A report arrived and could be read; another may follow immediately.
    Data,
    /// Nothing pending. The device is alive but has nothing to say.
    Empty,
    /// Something arrived, but it is not a report.
    ///
    /// A report ID the device's own descriptor does not declare. Some
    /// devices answer a read with no pending data with such an ID (e.g.
    /// 255 or 0) instead of with length 0 as HID over I2C specifies.
    ///
    /// This carries no information. It must neither keep the drain loop
    /// going nor count as evidence against the interrupt pin.
    Junk,
    /// The bus no longer answers.
    Dead,
}

// ── The gate on the interrupt pin ────────────────────────────────────
//
// One mapping per GPIO block, not per device: several devices often share
// one block, and `MAX_MMIO_MAPS` is four.
const MAX_GPIO_MAPS: usize = 2;
/// `(base, pages, handle)` per mapped block, and how many are in use.
static GPIO_MAPS: Single<([(u32, u32, i32); MAX_GPIO_MAPS], usize)> =
    Single::new(([(0, 0, -1); MAX_GPIO_MAPS], 0));

fn map_gpio_page(base: u32, pages: u32) -> i32 {
    GPIO_MAPS.with(|(maps, n)| {
        for (b, p, h) in maps.iter().take(*n) {
            if *b == base && *p >= pages { return *h; }
        }
        let h = npk_sys::mmio_map_phys(0, base as i32, pages as i32);
        if h >= 0 && *n < MAX_GPIO_MAPS {
            maps[*n] = (base, pages, h);
            *n += 1;
        }
        h
    })
}

/// Arm the gate, or log why not.
///
/// Every refusal is logged; a driver silently polling blind looks the same
/// as one that does not.
fn arm_gate(d: &i2c_hid_core::discover::HidDevice) -> Gate {
    use i2c_hid_core::gpio;

    let addr = d.slave_address;
    let Some(&pin) = d.gpio_pins.first() else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: no GpioInt in _CRS — reading blind every 5 ms"));
        return Gate::Blind;
    };
    let Some(g) = d.gpio_controller.as_ref() else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GpioInt names \"{}\", which is not in the namespace — \
             reading blind", d.gpio_source));
        return Gate::Blind;
    };
    // The register layout is AMD-specific. A different block at the same
    // place has something else there, and a guessed bit 16 would be worse
    // than no check at all.
    if !gpio::is_amd_block(&g.ids) {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GPIO block [{}] is not one whose registers we know — \
             reading blind", g.ids.join(",")));
        return Gate::Blind;
    }
    // An edge cannot be polled: it is over by the time anyone looks. Only a
    // level stays asserted until the report is read.
    if !d.gpio_level_triggered() {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GpioInt is edge-triggered — a level is what can be \
             polled, reading blind"));
        return Gate::Blind;
    }
    let Some(w) = gpio::pin_window(g.mmio_base, g.mmio_len, pin) else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: pin {pin} lies outside {:#010x}+{:#x} — reading blind",
            g.mmio_base, g.mmio_len));
        return Gate::Blind;
    };
    let handle = map_gpio_page(w.map_base, w.pages);
    if handle < 0 {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GPIO MMIO {:#010x} not mappable — reading blind",
            w.map_base));
        return Gate::Blind;
    }
    let active_low = d.gpio_active_low();
    let v = npk_sys::mmio_read32(handle, w.reg_off as i32) as u32;
    if v == u32::MAX {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: pin register reads all ones — nobody answered there, \
             reading blind"));
        return Gate::Blind;
    }
    logln(&alloc::format!(
        "[i2c-hid] {addr:#04x}: gating on GPIO pin {pin} ({:#010x}+{:#x}, active-{}, \
         now {}) — the bus is only touched when it says so",
        w.map_base, w.reg_off,
        if active_low { "low" } else { "high" },
        if gpio::asserted(v, active_low) { "asserted" } else { "idle" }));
    Gate::Pin { handle, reg_off: w.reg_off, active_low, skipped: 0, contradictions: 0, proved: false }
}

/// Is something pending right now? Without a gate the answer is always yes.
fn gate_asserted_now(l: &mut Live) -> bool {
    match &l.gate {
        Gate::Blind => true,
        Gate::Pin { handle, reg_off, active_low, .. } => {
            let v = npk_sys::mmio_read32(*handle, *reg_off as i32) as u32;
            i2c_hid_core::gpio::asserted(v, *active_low)
        }
    }
}

/// Should this round read, and did the pin say no?
fn gate_check(l: &mut Live) -> (bool, bool) {
    let Gate::Pin { handle, reg_off, active_low, skipped, proved, .. } = &mut l.gate else {
        return (true, false);
    };
    let v = npk_sys::mmio_read32(*handle, *reg_off as i32) as u32;
    if i2c_hid_core::gpio::asserted(v, *active_low) {
        *skipped = 0;
        return (true, false);
    }
    *skipped += 1;
    let every = if *proved { GATE_CROSS_CHECK_PROVED } else { GATE_CROSS_CHECK_ROUNDS };
    if *skipped >= every {
        *skipped = 0;
        (true, true)
    } else {
        (false, true)
    }
}

/// Evaluate the cross-check.
///
/// The gate is disabled only by a contradiction: a report that arrived
/// although the pin reported nothing. Nothing arriving proves nothing; an
/// untouched touchpad is silent.
fn gate_verdict(l: &mut Live, gate_said_no: bool, got_data: bool) {
    if !got_data { return; }
    let addr = l.addr;
    let n = match &mut l.gate {
        Gate::Blind => return,
        Gate::Pin { contradictions, proved, .. } => {
            if !gate_said_no {
                // A correct announcement clears the contradictions.
                //
                // A single contradiction can be a race: the finger lands
                // microseconds after the pin was read. A wrong gate
                // contradicts on every cross-check and never announces
                // correctly; only that should disable it.
                *contradictions = 0;
                if !*proved {
                    *proved = true;
                    dbgln(&alloc::format!(
                        "[i2c-hid] {addr:#04x}: the pin announced a report — the gate holds"));
                }
                return;
            }
            *contradictions += 1;
            *contradictions
        }
    };
    if n >= GATE_MAX_CONTRADICTIONS {
        l.gate = Gate::Blind;
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: {n} reports in a row arrived while the pin said \
             nothing — the gate is wrong, back to reading blind"));
    } else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: a report arrived while the pin said nothing \
             ({n}/{GATE_MAX_CONTRADICTIONS})"));
    }
}

/// Fetch one input report and feed it as pointer motion or gesture.
///
/// A mouse reports motion, a touchpad reports positions; motion is the
/// difference to the previous position, and the first touch yields none.
/// Gestures are derived from the number of contacts and their motion.
fn poll_live(l: &mut Live, buf: &mut [u8]) -> Step {
    use i2c_hid_core::{hid, report};
    let r = match hid::get_input(&mut l.bus, &l.dw, l.addr, &l.desc, buf) {
        Ok(Some(r)) => r,
        Ok(None) => return Step::Empty,
        Err(e) => {
            // Keep the reason: a Timeout and an AddrNack look alike from
            // outside but differ in cost by three orders of magnitude; one
            // returns immediately, the other holds xfer up to one second.
            if l.err_logged < 8 {
                l.err_logged += 1;
                logln(&alloc::format!(
                    "[i2c-hid] {:#04x}: input read failed: {e:?}", l.addr));
            }
            return Step::Dead;
        }
    };
    let (id, data) = if l.uses_ids && !r.is_empty() { (r[0], &r[1..]) } else { (0u8, r) };

    // Pick the decoder for this report ID. If none matches, log the ID once;
    // that is the missing information when nothing moves.
    let Some(d) = l.decoders.iter().find(|d| d.rid == id) else {
        if l.unknown_logged < 3 {
            l.unknown_logged += 1;
            logln(&alloc::format!(
                "[i2c-hid]   report id {id} arrived, {} bytes — the descriptor does not \
                 declare that id; taking it as nothing", r.len()));
        }
        return Step::Junk;
    };
    let (mode, btn) = (&d.mode, &d.btn);
    l.seen += 1;
    if Some(id) == l.touch_rid {
        if !l.saw_touch {
            l.saw_touch = true;
            dbgln(&alloc::format!(
                "[i2c-hid] {:#04x}: touchpad report {id} is live — precision mode took",
                l.addr));
        }
    } else {
        l.other_seen += 1;
    }

    if l.raw_logged < 4 {
        l.raw_logged += 1;
        dbgln(&alloc::format!("[i2c-hid] {:#04x} in {id}: {:02x?}", l.addr, data));
    }

    // Remember the physical button state, but only from a report that
    // carries buttons. A device reporting its button in a separate report
    // would otherwise have it silently cleared by the next contact report,
    // and pinning under the press would have no effect.
    if !btn.is_empty() {
        let mut buttons = 0i32;
        for (i, f) in btn.iter().enumerate() {
            if report::extract(data, f) != 0 { buttons |= 1 << i; }
        }
        l.hw_buttons = buttons;
    }
    let buttons = l.hw_buttons;

    let (dx, dy, scroll, hscroll, tap) = match mode {
        Mode::Mouse { fx, fy, wheel } => {
            let x = report::extract(data, fx);
            let y = report::extract(data, fy);
            // The wheel is always relative: detents, not a position.
            let s = wheel.as_ref().map(|w| report::extract(data, w)).unwrap_or(0);
            if fx.relative {
                (x, y, s, 0, 0)
            } else {
                let d = if l.have_ref { (x - l.rx, y - l.ry) } else { (0, 0) };
                l.have_ref = true;
                l.rx = x; l.ry = y;
                (d.0, d.1, s, 0, 0)
            }
        }
        Mode::Touchpad { contacts, count } => {
            use i2c_hid_core::gesture;
            let cc = count.as_ref().map(|c| report::extract(data, c)).unwrap_or(-1);
            let mut present = [(0i32, 0i32, 0i32); 8];
            let mut np = 0usize;
            for (i, c) in contacts.iter().enumerate() {
                if report::extract(data, &c.tip) != 0 && np < present.len() {
                    // Without an identifier field the slot is the ID.
                    let cid = c.id.as_ref()
                        .map(|f| report::extract(data, f))
                        .unwrap_or(i as i32);
                    present[np] =
                        (cid, report::extract(data, &c.x), report::extract(data, &c.y));
                    np += 1;
                }
            }
            let now_ms = { let t = npk_sys::now_us(); if t < 0 { 0 } else { t as u64 / 1000 } };
            // The button is passed along: a pressed pad pins the fingers,
            // and a touch under the button is not a tap. Both belong in the
            // tracker, where they are tested.
            match l.track.feed(cc, &present[..np], contacts.len(), now_ms, buttons != 0) {
                gesture::Out::Pending => (0, 0, 0, 0, 0),
                gesture::Out::Frame { n, gesture, dx, dy, scroll, hscroll, tap } => {
                    if n > 0 && l.touch_logged < 3 {
                        l.touch_logged += 1;
                        dbgln(&alloc::format!(
                            "[i2c-hid]   frame: {n} finger(s), contact-count {cc}, \
                             gesture {gesture}, {:?}",
                            &l.track.frame()[..n.min(2)]));
                    }
                    if (scroll != 0 || hscroll != 0) && l.scroll_logged < 3 {
                        l.scroll_logged += 1;
                        dbgln(&alloc::format!(
                            "[i2c-hid]   scroll: {scroll} up, {hscroll} right"));
                    }
                    if tap > 0 && l.tap_logged < 2 {
                        l.tap_logged += 1;
                        dbgln(&alloc::format!("[i2c-hid]   tap: {tap} finger(s)"));
                    }
                    (dx, dy, scroll, hscroll, tap)
                }
            }
        }
    };

    // State first, then the click, then the motion.
    //
    // The order matters: on the second tap of a series the tracker reports
    // "release held button" and "one full click" in the same frame. If the
    // click came first it would fall inside the still-pressed button and
    // the compositor would see only one.
    push_buttons(l);
    if tap > 0 {
        let b = 1i32 << (tap - 1);
        npk_sys::pointer_inject(0, 0, l.last_buttons | b, 0, 0);
        npk_sys::pointer_inject(0, 0, l.last_buttons, 0, 0);
    }
    if dx != 0 || dy != 0 || scroll != 0 || hscroll != 0 {
        npk_sys::pointer_inject(dx, dy, l.last_buttons, scroll, hscroll);
    }
    Step::Data
}

/// Report the button state if it changed.
///
/// The only place that composes it. It has two sources — the physical
/// buttons of the last report and the button a tap is currently holding —
/// and two places combining them separately would be two semantics.
fn push_buttons(l: &mut Live) {
    let want = l.hw_buttons | l.track.hold() as i32;
    if want != l.last_buttons {
        npk_sys::pointer_inject(0, 0, want, 0, 0);
        l.last_buttons = want;
    }
}
