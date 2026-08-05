//! System intents: status, time, help, about, caps, audit, halt, set/get/config

use crate::{kprint, kprintln};
use crate::capability::{self, Vault};

pub fn intent_status(vault: &Vault) {
    let (active_caps, max_caps) = vault.stats();
    let (free_frames, free_mb) = crate::memory::stats();
    let uptime = crate::interrupts::uptime_secs();
    let audit_count = crate::audit::total_count();

    kprintln!();
    kprintln!("  nopeekOS v{} – AI-native Operating System", env!("CARGO_PKG_VERSION"));
    kprintln!("  ──────────────────────────────────────────");
    kprintln!("  Uptime:        {}m {}s", uptime / 60, uptime % 60);
    kprintln!("  Phase:         2 (Capability Enforcement)");
    let cores = crate::smp::per_core::core_count();
    let wakeup = if crate::smp::per_core::has_mwait() { "MWAIT" } else { "HLT" };
    match crate::smp::per_core::dedicated_vm_core() {
        Some(c) => kprintln!(
            "  CPU:           x86_64, {} cores (work-stealing, {}; core {} → microvm)",
            cores, wakeup, c
        ),
        None => kprintln!(
            "  CPU:           x86_64, {} cores (work-stealing, {})",
            cores, wakeup
        ),
    }
    let (heap_used, heap_total) = crate::heap::stats();
    let (huge_pages, small_pages) = crate::paging::stats();
    kprintln!("  Memory:        {} MB free ({} frames)", free_mb, free_frames);
    kprintln!("  Heap:          {} KB / {} MB", heap_used / 1024, heap_total / (1024 * 1024));
    kprintln!("  Paging:        {} x 2MB + {} x 4KB, NX enabled", huge_pages, small_pages);
    kprintln!("  Capabilities:  {}/{} active", active_caps, max_caps);
    kprintln!("  Audit log:     {} events", audit_count);
    kprintln!("  WASM Runtime:  wasmi (interpreter)");
    if let Some(cap) = crate::blkdev::capacity() {
        let mb = (cap * 512) / (1024 * 1024);
        let dev = if crate::nvme::is_available() { "NVMe" } else { "virtio-blk" };
        kprintln!("  Block device:  {} MB ({} sectors, {})", mb, cap, dev);
    } else {
        kprintln!("  Block device:  none");
    }
    if let Some(mac) = crate::netdev::mac() {
        let ip = crate::net::arp::our_ip();
        kprintln!("  Network:       {}.{}.{}.{} ({:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            ip[0], ip[1], ip[2], ip[3], mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    } else {
        kprintln!("  Network:       none");
    }
    if let Some((_, free, objects, generation)) = crate::npkfs::stats() {
        kprintln!("  npkFS:         {} objects, {} free blocks (gen {})", objects, free, generation);
    } else {
        kprintln!("  npkFS:         not mounted");
    }
    kprintln!();
}

/// `cores` / `cpu` — trustworthy per-core CPU instrumentation.
///
/// Diagnosis step 0 for the scheduler rework: the WASM `top` is broken
/// (self-reported busy-TSC can't see spinners; APERF/MPERF absent on
/// AMD/qemu). This measures the opposite, directly: it double-samples
/// the per-core HALTED-cycle counters (recorded at every HLT/MWAIT site)
/// over a fixed window and reports the ground truth.
///
///   BUSY% = 100 − halted%  → a spinning core never halts → shows ~100%
///   HALTS/s + avg residency → many short halts = spurious-wake spin;
///                             few long halts = healthy deep idle.
///
/// Output goes to serial via kprintln (primary I/O), bypassing the
/// broken WASM top entirely.
pub fn intent_cores() {
    let cores = crate::smp::per_core::core_count().min(256);
    let tsc_hz = crate::interrupts::tsc_freq().max(1);
    let tsc_per_us = (tsc_hz / 1_000_000).max(1);

    // Snapshot 1
    let mut s0 = [(0u64, 0u64); 256];
    let mut w0 = [[0u64; crate::smp::per_core::WAKE_CAUSES]; 256];
    for c in 0..cores {
        s0[c] = crate::smp::per_core::halt_snapshot(c);
        w0[c] = crate::smp::per_core::wake_snapshot(c);
    }
    let vx0 = crate::microvm::cpu::vm_exit_snapshot();
    let io0 = crate::microvm::cpu::io_port_snapshot();
    let wk0 = crate::microvm::devices::net_dataplane::wake_snapshot();
    let kw0 = crate::smp::fiber::kick_wait_snapshot();
    let kl0 = crate::smp::fiber::kick_latency_snapshot(); // clears max
    let dk0 = crate::microvm::devices::nat::decoupled_kick_count();
    let rf0 = crate::microvm::devices::nat::ringfull_kick_count();
    let rh0 = crate::microvm::devices::nat::rx_health_snapshot();
    let gt0 = crate::microvm::devices::nat::guest_timer_count();
    let tx0 = crate::microvm::devices::nat::tx_stats();
    let rp0 = crate::microvm::devices::net_dataplane::rx_pass_stats(); // clears gap_max
    let (cy0, gcy0) = crate::microvm::cpu::vm_cycle_snapshot();
    let wall0 = crate::interrupts::rdtsc();

    // Sample window. Idle Core 0 honestly with HLT (the normal shell-idle
    // path) instead of busy-waiting, so Core 0's own halt counter advances
    // and its BUSY% reflects reality rather than this command spinning.
    let window_ms: u64 = 500;
    let deadline = wall0 + window_ms * (tsc_hz / 1000);
    while crate::interrupts::rdtsc() < deadline {
        let t0 = crate::interrupts::rdtsc();
        // SAFETY: ring-0, IRQs enabled in the shell loop — the 100 Hz
        // timer wakes us within ~10 ms to re-check the deadline.
        unsafe { core::arch::asm!("hlt"); }
        crate::smp::per_core::record_halt(
            0, crate::interrupts::rdtsc().saturating_sub(t0));
    }

    // Snapshot 2
    let wall1 = crate::interrupts::rdtsc();
    let mut s1 = [(0u64, 0u64); 256];
    let mut w1 = [[0u64; crate::smp::per_core::WAKE_CAUSES]; 256];
    for c in 0..cores {
        s1[c] = crate::smp::per_core::halt_snapshot(c);
        w1[c] = crate::smp::per_core::wake_snapshot(c);
    }
    let vx1 = crate::microvm::cpu::vm_exit_snapshot();
    let io1 = crate::microvm::cpu::io_port_snapshot();
    let wk1 = crate::microvm::devices::net_dataplane::wake_snapshot();
    let kw1 = crate::smp::fiber::kick_wait_snapshot();
    let kl1 = crate::smp::fiber::kick_latency_snapshot();
    let dk1 = crate::microvm::devices::nat::decoupled_kick_count();
    let rf1 = crate::microvm::devices::nat::ringfull_kick_count();
    let rh1 = crate::microvm::devices::nat::rx_health_snapshot();
    let gt1 = crate::microvm::devices::nat::guest_timer_count();
    let tx1 = crate::microvm::devices::nat::tx_stats();
    let rp1 = crate::microvm::devices::net_dataplane::rx_pass_stats(); // gap_max = window peak
    let (cy1, gcy1) = crate::microvm::cpu::vm_cycle_snapshot();
    let dwall = wall1.saturating_sub(wall0).max(1);

    let vmcore = crate::smp::per_core::dedicated_vm_core();

    kprintln!();
    kprintln!("  Per-core CPU (idle-measured, {} ms window, idle=HLT+100Hz timer)", window_ms);
    kprintln!("  ─────────────────────────────────────────────────────");
    kprintln!("  CORE   BUSY%   HALTS/s   AVG-RESIDENCY   QUEUE  ROLE");
    for c in 0..cores {
        let dhalt = s1[c].0.saturating_sub(s0[c].0);
        let dcount = s1[c].1.saturating_sub(s0[c].1);

        // True busy% = 100 − (halted cycles / wall cycles).
        let halt_pct = ((dhalt as u128) * 100 / (dwall as u128)) as u64;
        let busy = 100u64.saturating_sub(halt_pct.min(100));

        // Halt entries per second (window is window_ms long).
        let halts_per_s = dcount * 1000 / window_ms;

        // Average residency per halt, in µs.
        let avg_us = if dcount > 0 { (dhalt / dcount) / tsc_per_us } else { 0 };

        let qlen = crate::smp::scheduler::queue_len(c);

        // ROLE: classify from the measured signals, not a hardcoded guess.
        let role: &str = if c == 0 {
            "core0 (kernel/irq/shell)"
        } else if Some(c) == vmcore {
            "microvm (dedicated)"
        } else if busy >= 90 && halts_per_s < 5 {
            "SPINNING (never halts!)"
        } else if halts_per_s > 200 && avg_us < 50 {
            "spin? (spurious wakes)"
        } else if crate::smp::per_core::is_active(c) {
            "running task"
        } else if busy < 5 {
            "idle (asleep)"
        } else {
            "worker"
        };

        if dcount > 0 {
            kprintln!("  {:>4}   {:>4}%   {:>7}   {:>10} us   {:>5}  {}",
                c, busy, halts_per_s, avg_us, qlen, role);
        } else {
            kprintln!("  {:>4}   {:>4}%   {:>7}   {:>13}   {:>5}  {}",
                c, busy, halts_per_s, "—", qlen, role);
        }

        // Wake-source breakdown: which cause returned each halt this
        // window. The decisive number is UNATTR = HALTS − Σcauses: large
        // here means the HLT returned with NO guest ISR — KVM resuming
        // the vCPU on a host event (host HZ tick) past the emulated HLT.
        // That is a QEMU/KVM artifact, not a bare-metal idle bug.
        let labels = crate::smp::per_core::WAKE_LABELS;
        let mut attributed = 0u64;
        // Build "cause=N/s" only for non-zero causes to keep it terse.
        // (kprintln has no String; print inline per cause.)
        kprint!("        wakes:");
        let mut any = false;
        for i in 0..labels.len() {
            let d = w1[c][i].saturating_sub(w0[c][i]);
            attributed = attributed.saturating_add(d);
            if d > 0 {
                kprint!(" {}={}/s", labels[i], d * 1000 / window_ms);
                any = true;
            }
        }
        if !any { kprint!(" (none)"); }
        let unattr = dcount.saturating_sub(attributed);
        kprintln!("  | UNATTR={}/s", unattr * 1000 / window_ms);
    }

    // VM-exit mix — only when a guest ran during the window. Tells us
    // WHY the dedicated core is busy: mmio-heavy = the guest is rendering
    // (legit); hlt/intr-heavy = idle spin (the run loop should yield/sleep).
    let vlabels = crate::microvm::cpu::VMEXIT_LABELS;
    let vtotal: u64 = (0..vlabels.len())
        .map(|i| vx1[i].saturating_sub(vx0[i]))
        .sum();
    if vtotal > 0 {
        kprint!("  VM-exits/s (dedicated guest):");
        for i in 0..vlabels.len() {
            let d = vx1[i].saturating_sub(vx0[i]);
            if d > 0 { kprint!(" {}={}", vlabels[i], d * 1000 / window_ms); }
        }
        kprintln!();
        // Break the `io` exit bucket down by port. During heavy RX this is
        // expected to be dominated by `pic` (the 8259 EOI, one outb 0x20 per
        // device IRQ since the guest runs noapic) — proving the io storm is
        // the interrupt-ack path, not the data path.
        let iolabels = crate::microvm::cpu::IO_PORT_LABELS;
        let iototal: u64 = (0..iolabels.len())
            .map(|i| io1[i].saturating_sub(io0[i]))
            .sum();
        if iototal > 0 {
            kprint!("    io-exit ports/s:");
            for i in 0..iolabels.len() {
                let d = io1[i].saturating_sub(io0[i]);
                if d > 0 { kprint!(" {}={}", iolabels[i], d * 1000 / window_ms); }
            }
            kprintln!();
        }
        // Net RX worker wakeup attribution: irq = event-driven (host RX MSI-X
        // woke it, ~µs); timeout = fell to the 2ms fallback (host IRQ did NOT
        // fire → silent polling = the cold-start floor); polled = no MSI-X.
        let (wi, wt, wp, ws) = (
            wk1.0.saturating_sub(wk0.0),
            wk1.1.saturating_sub(wk0.1),
            wk1.2.saturating_sub(wk0.2),
            wk1.3.saturating_sub(wk0.3),
        );
        if wi + wt + wp + ws > 0 {
            kprintln!("    net worker wakes/s: irq={} timeout={} polled={} busy={}",
                      wi * 1000 / window_ms, wt * 1000 / window_ms,
                      wp * 1000 / window_ms, ws * 1000 / window_ms);
        }
        // BSP consumer park: kicked = the worker's kick woke it (event-driven);
        // timeout = it fell to the 2ms fallback = the typical ~3ms cold floor.
        let (kk, kt) = (kw1.0.saturating_sub(kw0.0), kw1.1.saturating_sub(kw0.1));
        // decoupled = kicks issued for STAGED-but-IRQ-suppressed RX (injected &&
        // !want_irq). These used to be lost wakes → the 2ms `timeout` stalls; now
        // they wake the vCPU. timeout should fall toward 0 as decoupled rises.
        let dk = dk1.saturating_sub(dk0);
        let rf = rf1.saturating_sub(rf0);
        if kk + kt > 0 {
            kprintln!("    bsp kick_wait/s: kicked={} timeout={} decoupled={} ringfull={}",
                      kk * 1000 / window_ms, kt * 1000 / window_ms,
                      dk * 1000 / window_ms, rf * 1000 / window_ms);
        }
        // kick→resume LATENCY (the irqfd-gap probe): how long from the worker's RX
        // kick to the parked BSP vCPU actually resuming. µs = IPI-prompt (3ms RTT
        // is elsewhere); ms = kicked-but-host-descheduled wake (nested oversub) =
        // the structural irqfd gap → the real per-packet-round-trip cost.
        let kln = kl1.1.saturating_sub(kl0.1);
        if kln > 0 {
            let avg_us = (kl1.0.saturating_sub(kl0.0)) / kln * 1_000_000 / tsc_hz.max(1);
            let max_us = kl1.2.saturating_mul(1_000_000) / tsc_hz.max(1);
            kprintln!("    bsp kick→resume: avg={}us max={}us (n={}/s)",
                      avg_us, max_us, kln * 1000 / window_ms);
        }
        // Bridge RX backpressure: drops/s = INBOUND_Q overflow (→ server retransmit
        // → cwnd collapse → the slow regime). If this climbs during a SLOW GET, the
        // lottery is server-cwnd-collapse from our drops, not park latency.
        let drops = rh1.0.saturating_sub(rh0.0);
        let injf = rh1.1.saturating_sub(rh0.1);
        let rxlat_us = rh1.2.saturating_mul(1_000_000) / tsc_hz.max(1);
        kprintln!("    net bridge backpressure: drops={}/s injfalse={}/s rxlat_max={}us",
                  drops * 1000 / window_ms, injf * 1000 / window_ms, rxlat_us);
        // Outbound TX rate (the b1-vs-b2 upload discriminator). Read TOGETHER with
        // the worker core's BUSY% above: high segs/s + worker pegged ~100% = the
        // SW-TSO emit pipeline is the cap (b1); the same Mbit with the worker idle
        // = cwnd × inflated bridge RTT (b2, an ACK-clock the emit can't lift).
        let txp = tx1.0.saturating_sub(tx0.0);
        let txb = tx1.1.saturating_sub(tx0.1);
        if txp > 0 {
            kprintln!("    net TX: {}segs/s avgseg={}B {}Mbit",
                      txp * 1000 / window_ms,
                      txb / txp,
                      txb * 8 / 1000 / window_ms);
        }
        // Full-path RX cadence (the rxlat/drops line above is BLIND in full mode).
        // batch = avg frames drained per non-empty pass; gap_max = peak µs between
        // passes. Decisive read: small batch + ~1.5ms gap = park-cadence (lever a,
        // RTT-bound); large batch (+ guest ring full) = receiver-drain (lever b).
        let rpf = rp1.0.saturating_sub(rp0.0);
        let rpc = rp1.1.saturating_sub(rp0.1);
        let gap_us = rp1.2.saturating_mul(1_000_000) / tsc_hz.max(1);
        if rpc > 0 {
            kprintln!("    net RX cadence: {}passes/s batch={}frames gap_max={}us",
                      rpc * 1000 / window_ms, rpf / rpc, gap_us);
        }
        // Effective guest HZ: the guest programs 1 kHz (CONFIG_HZ=1000); injected
        // only while VMRUN runs, so a parky (slow) connection sees <1000 = the
        // timer freezing under the 2ms parks = the "1000 vs 100" lottery.
        let gt = gt1.saturating_sub(gt0);
        kprintln!("    guest timer/s (effective HZ): {}", gt * 1000 / window_ms);
    }
    // Host-time breakdown: where the dedicated guest cores actually SPENT
    // their cycles this window. guest% = in VMRESUME (the guest really ran);
    // a high mmio/io% with low guest% PROVES the host burns the core on
    // exit-handling (mmio decode / PIC EOI) and the guest is starved — its
    // "0% CPU" is because it never gets scheduled, not because nothing runs.
    let gdelta = gcy1.saturating_sub(gcy0);
    let cdelta: u64 = (0..vlabels.len()).map(|i| cy1[i].saturating_sub(cy0[i])).sum();
    let ctotal = gdelta + cdelta;
    if ctotal > 0 {
        kprint!("  Host cycles: guest={}%", gdelta * 100 / ctotal);
        for i in 0..vlabels.len() {
            let d = cy1[i].saturating_sub(cy0[i]);
            if d * 100 / ctotal > 0 { kprint!(" {}={}%", vlabels[i], d * 100 / ctotal); }
        }
        kprintln!();
        kprintln!("  (guest%=in VMRESUME; high mmio/io% + low guest% = host burns the");
        kprintln!("   core on exit-handling, guest starved -> its '0% CPU' is no time given)");
    }
    kprintln!();
    kprintln!("  Read: BUSY%=100−halted. A core pegged at 100% with 0 HALTS/s");
    kprintln!("  is SPINNING (the idle-100% bug). Many HALTS/s + tiny residency");
    kprintln!("  = waking spuriously instead of staying asleep. Healthy idle =");
    kprintln!("  low BUSY%, few HALTS/s, long residency.");
    kprintln!("  wakes: which cause returned each halt. UNATTR = HALTS−Σcauses;");
    kprintln!("  large UNATTR (Core 0) = HLT returned with no guest ISR = KVM/");
    kprintln!("  host-tick artifact (QEMU), not a bare-metal idle bug.");
    kprintln!();
}

pub fn intent_history() {
    super::print_active_history();
}

/// `akku` / `battery` — Smart-Battery diagnostic. Shows whether the i801
/// SMBus controller was found, dumps the raw SBS registers read from the
/// pack at address 0x0B, and prints the decoded charge + status. Lets us
/// tell "no controller" from "controller but no battery on the bus" from
/// "battery present but odd values" without a serial cable.
pub fn intent_battery() {
    const SBS_ADDR: u8 = 0x0B;
    kprintln!();
    kprintln!("  Battery (Smart Battery over SMBus)");
    kprintln!("  ──────────────────────────────────");

    match crate::smbus::base() {
        Some(b) => kprintln!("  SMBus i801:    present @ I/O 0x{:04x}", b),
        None => {
            kprintln!("  SMBus i801:    NOT FOUND (no PCI class 0C05)");
            kprintln!("  → no controller → no battery readout possible");
            return;
        }
    }

    // Raw register dump (each is a 16-bit SMBus word read). None = NAK /
    // no device answering at that address/register.
    let regs: [(&str, u8); 6] = [
        ("RelStateOfCharge 0x0D", 0x0D),
        ("BatteryStatus    0x16", 0x16),
        ("AverageCurrent   0x0A", 0x0A),
        ("RemainingCap     0x0F", 0x0F),
        ("FullChargeCap    0x10", 0x10),
        ("Voltage          0x09", 0x09),
    ];
    let mut sbs_ok = false;
    kprintln!("  Raw @ addr 0x0B:");
    for (name, reg) in regs {
        match crate::smbus::read_word(SBS_ADDR, reg) {
            Some(v) => { sbs_ok = true; kprintln!("    {} = 0x{:04x} ({})", name, v, v); }
            None    => kprintln!("    {} = NAK (no response)", name),
        }
    }
    if sbs_ok {
        match crate::battery::read() {
            Some(b) => {
                let st = match b.status {
                    crate::battery::ChargeStatus::Charging    => "charging",
                    crate::battery::ChargeStatus::Discharging => "discharging",
                    crate::battery::ChargeStatus::Full        => "full",
                    crate::battery::ChargeStatus::PluggedIdle => "plugged, not charging",
                };
                kprintln!("  Decoded:       {}% — {}", b.percent, st);
            }
            None => kprintln!("  Decoded:       read failed"),
        }
        return;
    }

    kprintln!("  → pack does not answer at 0x0B (behind the EC).");
    intent_ec_battery_dump();
}

/// Dump the EC's 256-byte RAM so we can reverse-engineer the battery
/// fields on this machine (HP Elite/Dragonfly stores charge as plain EC-RAM
/// fields: remaining cap, full cap, status — read by the DSDT's _BST). Find
/// the offset whose byte ≈ the known charge %, and the 16-bit capacity pair.
fn intent_ec_battery_dump() {
    kprintln!();
    kprintln!("  EC RAM dump (0x00..0xFF) — find the battery fields:");
    let mut ram = [0u8; 256];
    let mut read_ok = false;
    for i in 0..256u16 {
        match crate::ec::read(i as u8) {
            Some(v) => { ram[i as usize] = v; read_ok = true; }
            None    => ram[i as usize] = 0xFF,
        }
    }
    if !read_ok {
        kprintln!("    EC not responding on ports 0x62/0x66 (timeout).");
        return;
    }
    // 16 bytes per row, hex.
    for row in 0..16usize {
        let b = &ram[row * 16..row * 16 + 16];
        kprintln!(
            "    {:02x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}  \
             {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
            row * 16,
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
    }
    // Leads: bytes that look like a charge % (1..=100).
    kprint!("  Candidate %% bytes (val 1..100):");
    for i in 0..256usize {
        if ram[i] >= 1 && ram[i] <= 100 {
            kprint!(" [0x{:02x}]={}", i, ram[i]);
        }
    }
    kprintln!();
    // Leads: 16-bit LE values in a plausible capacity range (mAh or mWh).
    kprint!("  Candidate capacities (u16 1000..65000):");
    for i in 0..255usize {
        let v = ram[i] as u16 | ((ram[i + 1] as u16) << 8);
        if v >= 1000 && v < 65000 {
            kprint!(" [0x{:02x}]={}", i, v);
        }
    }
    kprintln!();

    // Decoded HP-EC battery — offsets from the DSDT Field(ECRM): BSEL/BFC_/
    // BRC_/BST_ (mAh). This is what the bar segment shows.
    let bsel = crate::ec::read(0x86).unwrap_or(0xff);
    let full = crate::ec::read_u16(0x8d).unwrap_or(0);
    let rem = crate::ec::read_u16(0xa1).unwrap_or(0);
    let bst = crate::ec::read(0x99).unwrap_or(0) & 0x0f;
    let watts = crate::ec::read(0xf9).unwrap_or(0);
    kprintln!("  Decoded (HP-EC): BSEL={} BRC_={} mAh BFC_={} mAh BST_=0x{:x} charger={}W",
        bsel, rem, full, bst, watts);
    match crate::battery::read() {
        Some(b) => {
            let st = match b.status {
                crate::battery::ChargeStatus::Charging    => "charging",
                crate::battery::ChargeStatus::Discharging => "on battery",
                crate::battery::ChargeStatus::Full        => "full",
                crate::battery::ChargeStatus::PluggedIdle => "plugged, not charging",
            };
            kprintln!("  → {}%  ({})", b.percent, st);
        }
        None => kprintln!("  → battery decode failed"),
    }
}

/// AML NameString length at `p` (handles root/parent prefixes + dual/multi
/// name prefixes; plain 4-char NameSeg otherwise).
fn aml_name_len(b: &[u8], p: usize) -> usize {
    match b.get(p).copied() {
        Some(0x5C) | Some(0x5E) => 1 + aml_name_len(b, p + 1),
        Some(0x2E) => 1 + 8,
        Some(0x2F) => 2 + (b.get(p + 1).copied().unwrap_or(0) as usize) * 4,
        _ => 4,
    }
}

fn dsdt_dump_range(b: &[u8], label: &str, start: usize, count: usize) {
    let end = (start + count).min(b.len());
    kprintln!("  --- {} @ 0x{:x}..0x{:x} ---", label, start, end);
    let mut i = start;
    while i < end {
        let row = (end - i).min(16);
        // offset + hex + ASCII (field NameSegs are ASCII, easy to spot)
        let mut line = alloc::format!("  {:05x}: ", i);
        for j in 0..16 {
            if j < row {
                line.push_str(&alloc::format!("{:02x} ", b[i + j]));
            } else {
                line.push_str("   ");
            }
        }
        line.push(' ');
        for j in 0..row {
            let c = b[i + j];
            line.push(if (0x20..0x7f).contains(&c) { c as char } else { '.' });
        }
        kprintln!("{}", line);
        i += 16;
    }
}

/// `dsdt` — dump only the battery-relevant AML: every EmbeddedControl
/// OperationRegion+Field (field NameSegs → EC byte offsets, e.g. BRC/BFC)
/// and the _BST/_BIF/_BIX methods. Small enough to copy from the console;
/// from this we map the real remaining/full-charge EC offsets.
pub fn intent_dsdt() {
    let Some((addr, len)) = crate::acpi::dsdt() else {
        kprintln!("[npk] DSDT not found");
        return;
    };
    let b = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    kprintln!("  DSDT @ 0x{:x}, len {} bytes", addr, len);

    // We now know EC0.BTST reads fields BSEL/BST_/BPR_/BRC_/BPV_ and BTIF
    // reads BDC_/BFC_/BDV_. We need their EC byte offsets → dump the
    // enclosing Field() definition(s). Find each field NameSeg, scan back to
    // the FieldOp (0x5B 0x81) that declares it, dump from there so the
    // bit-offset accumulation (incl. Offset() skips) is visible from the top.
    let mut dumped: [usize; 8] = [usize::MAX; 8];
    let mut nd = 0;
    for (name, seg) in [
        ("Field(BSEL)", [0x42u8, 0x53, 0x45, 0x4C]),
        ("Field(BRC_)", [0x42u8, 0x52, 0x43, 0x5F]),
        ("Field(BFC_)", [0x42u8, 0x46, 0x43, 0x5F]),
    ] {
        // first occurrence of the NameSeg
        let mut k = 0;
        let mut at = usize::MAX;
        while k + 4 < len {
            if b[k..k + 4] == seg { at = k; break; }
            k += 1;
        }
        if at == usize::MAX { kprintln!("  {}: not found", name); continue; }
        // scan back for the FieldOp 0x5B 0x81
        let mut s = at;
        let lo = at.saturating_sub(2048);
        while s > lo {
            if b[s] == 0x5B && b[s + 1] == 0x81 { break; }
            s -= 1;
        }
        if !(b[s] == 0x5B && b[s + 1] == 0x81) {
            // no FieldOp found nearby — just dump around the name
            dsdt_dump_range(b, name, at.saturating_sub(8), 256);
            continue;
        }
        if nd < dumped.len() && dumped[..nd].contains(&s) { continue; } // dedup
        dsdt_dump_range(b, name, s, 1024);
        if nd < dumped.len() { dumped[nd] = s; nd += 1; }
    }
}

/// `dsdt full` — base64-dump the entire DSDT to the console, framed by
/// markers, so the aml.wasm interpreter dev-harness can reconstruct the exact
/// bytes the kernel sees (cross-check against Linux's acpidump). Generic ACPI
/// diagnostic — no device-specific logic.
pub fn intent_dsdt_full() {
    let Some((addr, len)) = crate::acpi::dsdt() else {
        kprintln!("[npk] DSDT not found");
        return;
    };
    let b = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    const A: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    kprintln!("---DSDT-BEGIN len={}---", len);
    // Encode in 48-byte input chunks (→ 64 base64 chars per line).
    let mut i = 0;
    while i < len {
        let chunk = (len - i).min(48);
        let mut line = alloc::string::String::with_capacity(64);
        let mut j = 0;
        while j < chunk {
            let b0 = b[i + j];
            let b1 = if j + 1 < chunk { b[i + j + 1] } else { 0 };
            let b2 = if j + 2 < chunk { b[i + j + 2] } else { 0 };
            line.push(A[(b0 >> 2) as usize] as char);
            line.push(A[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            line.push(if j + 1 < chunk { A[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char } else { '=' });
            line.push(if j + 2 < chunk { A[(b2 & 0x3f) as usize] as char } else { '=' });
            j += 3;
        }
        kprintln!("{}", line);
        i += chunk;
    }
    kprintln!("---DSDT-END---");
}

/// `dsdt send <ip> <port>` — stream the raw DSDT bytes over TCP to a
/// `nc -l <port>` listener (exact bytes, no base64, no terminal-mirror ring
/// overflow). Paced in small chunks so the NIC TX ring drains. The DSDT
/// carries its own length at header bytes 4..8, so the receiver can self-verify
/// the transfer is complete. Generic ACPI diagnostic.
pub fn intent_dsdt_send(ip: [u8; 4], port: u16) {
    let Some((addr, len)) = crate::acpi::dsdt() else {
        kprintln!("[npk] DSDT not found");
        return;
    };
    let b = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    kprintln!("[npk] dsdt send → {}.{}.{}.{}:{} ({} bytes)",
        ip[0], ip[1], ip[2], ip[3], port, len);

    let handle = match crate::net::tcp::connect(ip, port) {
        Ok(h) => h,
        Err(_) => { kprintln!("[npk] connect failed (is `nc -l {}` running?)", port); return; }
    };

    let mut off = 0usize;
    while off < len {
        let end = (off + 1024).min(len);
        if crate::net::tcp::send(handle, &b[off..end]).is_err() {
            kprintln!("[npk] send failed at offset {}", off);
            let _ = crate::net::tcp::close(handle);
            return;
        }
        off = end;
        // Pace ~10 ms so the (fire-and-forget) segments don't overrun the ring.
        let t0 = crate::interrupts::ticks();
        while crate::interrupts::ticks() == t0 {}
    }

    let _ = crate::net::tcp::close(handle);
    kprintln!("[npk] dsdt sent ({} bytes); verify: filesize == u32(bytes[4..8])", len);
}

pub fn intent_uptime() {
    let secs = crate::interrupts::uptime_secs();
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        kprintln!("up {}d {}h {}m {}s", days, hours, mins, s);
    } else if hours > 0 {
        kprintln!("up {}h {}m {}s", hours, mins, s);
    } else {
        kprintln!("up {}m {}s", mins, s);
    }
}

pub fn intent_gpu(args: &str) {
    match args.trim() {
        "dump" | "regs" => {
            crate::gpu::dump_native();
        }
        "test-pll" | "test" => {
            // Test PLL re-lock with firmware values (will kill display!)
            kprintln!("[npk] WARNING: This will disable the display!");
            kprintln!("[npk] Log will be saved after test.");

            let _pre_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            crate::gpu::test_pll();

            let log = crate::serial::stop_capture();
            crate::serial::start_capture();
            let log_name = crate::gpu::next_log_name();
            let _ = crate::npkfs::store(&log_name, log.as_bytes(), [0u8; 32]);
            kprintln!("[npk] Log saved: {}", log_name);
        }
        "init" | "activate" => {
            if crate::gpu::is_native() {
                kprintln!("[npk] GPU: native driver already active ({})", crate::gpu::driver_name());
                return;
            }
            if !crate::gpu::native_detected() {
                kprintln!("[npk] GPU: no native GPU detected");
                return;
            }

            // Capture serial output during init (survives black screen)
            // Stop normal capture, start fresh for GPU init
            let _pre_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            kprintln!("[npk] GPU: activating native driver...");
            let result = crate::gpu::activate_native();

            // Save GPU init log to npkFS (readable after reboot)
            let gpu_log = crate::serial::stop_capture();
            // Restore pre-existing capture
            crate::serial::start_capture();

            // Store log in npkFS (unencrypted, no cap needed — use zero cap)
            let log_data = alloc::format!("{}\n--- GPU INIT RESULT: {:?} ---\n", gpu_log,
                result.as_ref().map(|fb| alloc::format!("OK {}x{}", fb.width, fb.height))
                    .unwrap_or_else(|e| alloc::format!("{:?}", e)));
            let log_name = crate::gpu::next_log_name();
            let _ = crate::npkfs::store(&log_name, log_data.as_bytes(), [0u8; 32]);
            kprintln!("[npk] Log saved: {}", log_name);

            match result {
                Ok(fb) => {
                    crate::framebuffer::init_from_gpu();
                    kprintln!("[npk] GPU: {}x{} active", fb.width, fb.height);
                }
                Err(e) => {
                    kprintln!("[npk] GPU: activation failed: {:?}", e);
                    kprintln!("[npk] GOP framebuffer unchanged");
                    kprintln!("[npk] Check log with 'list gpu'");
                }
            }
        }
        "4k" | "4k30" | "4k60" => {
            if !crate::gpu::is_native() {
                kprintln!("[npk] GPU: native driver not active (run 'gpu init' first)");
                return;
            }

            let hz: u8 = if args.trim() == "4k60" { 60 } else { 30 };

            let _pre_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            kprintln!("[npk] GPU: switching to 4K@{}Hz...", hz);
            let result = crate::gpu::set_mode(3840, 2160, hz);

            let gpu_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            let log_data = alloc::format!("{}\n--- GPU MODE RESULT: {:?} ---\n", gpu_log,
                result.as_ref().map(|fb| alloc::format!("OK {}x{}", fb.width, fb.height))
                    .unwrap_or_else(|e| alloc::format!("{:?}", e)));
            let log_name = crate::gpu::next_log_name();
            let _ = crate::npkfs::store(&log_name, log_data.as_bytes(), [0u8; 32]);
            kprintln!("[npk] Log saved: {}", log_name);

            // Always reinit console — display hardware is already at new mode
            // even if pipe re-enable timed out
            crate::framebuffer::init_from_gpu();
            match result {
                Ok(fb) => {
                    kprintln!("[npk] GPU: {}x{} active", fb.width, fb.height);
                }
                Err(e) => {
                    kprintln!("[npk] GPU: mode switch partial: {:?} (display may work)", e);
                    kprintln!("[npk] Check log with 'list gpu'");
                }
            }
        }
        "blit init" => {
            if !crate::gpu::is_native() {
                kprintln!("[npk] GPU: native driver not active (run 'gpu init' first)");
                return;
            }
            if crate::gpu::supports_blit() {
                kprintln!("[npk] BCS: already initialized");
                return;
            }
            kprintln!("[npk] Initializing BCS blitter engine...");
            if crate::gpu::init_blit_engine() {
                kprintln!("[npk] BCS: blitter engine ready");
                // Map shadow buffers into GGTT for GPU blit
                if let Some((phys_a, phys_b, pages)) = crate::framebuffer::shadow_phys_info() {
                    if pages > 0 {
                        crate::gpu::map_shadows_for_blit(phys_a, phys_b, pages);
                        let (ga, gb) = crate::gpu::shadow_ggtt();
                        crate::framebuffer::set_shadow_ggtt(ga, gb);
                        kprintln!("[npk] BCS: shadow A GGTT={:#x}, shadow B GGTT={:#x}", ga, gb);
                    }
                }
            } else {
                kprintln!("[npk] BCS: init failed");
            }
        }
        "blit test" => {
            if !crate::gpu::supports_blit() {
                kprintln!("[npk] BCS: not initialized (run 'gpu blit init')");
                return;
            }
            crate::gpu::test_blit();
        }
        "blit status" | "blit" => {
            kprintln!("  BCS:      {}", if crate::gpu::supports_blit() { "ready" } else { "not initialized" });
            let (ga, gb) = crate::gpu::shadow_ggtt();
            if ga != 0 {
                kprintln!("  Shadow A: GGTT {:#x}", ga);
                kprintln!("  Shadow B: GGTT {:#x}", gb);
            }
            let fb_ggtt = crate::gpu::fb_ggtt_offset();
            if fb_ggtt != 0 {
                kprintln!("  FB GGTT:  {:#x}", fb_ggtt);
            }
        }
        "status" | "" => {
            kprintln!("  Driver:   {}", crate::gpu::driver_name());
            kprintln!("  Native:   {}", if crate::gpu::is_native() { "yes" } else { "no (GOP)" });
            if let Some(fb) = crate::gpu::framebuffer_info() {
                kprintln!("  Mode:     {}x{} {}bpp", fb.width, fb.height, fb.bpp);
                kprintln!("  FB addr:  {:#x}", fb.addr);
                kprintln!("  Pitch:    {} bytes ({} KB/line)", fb.pitch, fb.pitch / 1024);
                let fb_mb = (fb.pitch as u64 * fb.height as u64) / (1024 * 1024);
                kprintln!("  FB size:  {} MB", fb_mb);
            }
            let hz = crate::gpu::current_hz();
            if hz > 0 {
                kprintln!("  Refresh:  {}Hz", hz);
            }
            kprintln!("  VSync:    {}", if crate::gpu::supports_flip() { "planned (PLANE_SURF double-buffer)" } else { "no (GOP)" });
            kprintln!("  Flip:     {}", if crate::gpu::supports_flip() { "hardware (PLANE_SURF)" } else { "CPU blit" });

            // BCS blitter status
            let bcs_ok = crate::gpu::supports_blit();
            kprintln!("  BCS:      {}", if bcs_ok { "active" } else { "off (probe failed)" });
            // Readback self-test: did the blit actually PAINT? Distinguishes
            // "copy broken" (verified=no) from "copy ok but scanout wrong"
            // (verified=yes but screen black) on Tiger Lake bring-up.
            kprintln!("  Verified: {} (readback={:#010x})",
                if crate::gpu::blit_verified() { "YES — blit paints" } else { "no — blit did not paint" },
                crate::gpu::blit_readback());
            if bcs_ok {
                let fb_ggtt = crate::gpu::fb_ggtt_offset();
                let (ga, gb) = crate::gpu::shadow_ggtt();
                let front = crate::framebuffer::front_ggtt();
                kprintln!("  FB GGTT:  {:#x}", fb_ggtt);
                kprintln!("  Shadow A: GGTT {:#x}", ga);
                kprintln!("  Shadow B: GGTT {:#x}", gb);
                kprintln!("  Front:    GGTT {:#x} ({})",
                    front, if front == ga { "A" } else if front == gb { "B" } else { "?" });
                kprintln!("  Blit:     GPU (XY_FAST_COPY_BLT)");
                let mouse = crate::xhci::mouse_available();
                kprintln!("  Cursor:   {}", if mouse { "GPU-composited (save-under)" } else { "none" });
            } else {
                kprintln!("  Blit:     CPU (memcpy)");
                let mouse = crate::xhci::mouse_available();
                kprintln!("  Cursor:   {}", if mouse { "MMIO overlay" } else { "none" });
            }

            // BCS register dump (always, for debug)
            if crate::gpu::is_native() {
                kprintln!("  --- BCS regs ---");
                crate::gpu::dump_bcs_regs();
            }

            // Shadow buffer info
            if let Some((pa, pb, pages)) = crate::framebuffer::shadow_phys_info() {
                kprintln!("  Shadow:   {} pages ({} MB) x2", pages, pages * 4 / 1024);
                kprintln!("  Phys A:   {:#x}", pa);
                kprintln!("  Phys B:   {:#x}", pb);
            }

            if let Some(name) = crate::gpu::native_gpu_name() {
                if !crate::gpu::is_native() {
                    kprintln!("  Pending:  {} (use 'gpu init')", name);
                }
            }
            let modes = crate::gpu::supported_modes();
            if !modes.is_empty() {
                kprintln!("  Modes:");
                for m in &modes {
                    kprintln!("    {}x{} @ {}Hz", m.width, m.height, m.hz);
                }
            }
        }
        _ => {
            kprintln!("Usage: gpu [status|init|4k|blit init|blit test|blit status]");
        }
    }
}

pub fn intent_shade(args: &str) {
    match args.trim() {
        "init" | "start" => {
            if crate::shade::is_active() {
                kprintln!("[npk] shade: already running");
                return;
            }
            // Destroy pre-shade sessions so terminals start clean
            for i in 0..8u8 { crate::intent::destroy_session(i); }
            crate::shade::init();
            crate::shade::render_frame();
            kprintln!("[npk] shade: compositor active (Mod+Enter for first window)");
        }
        "demo" => {
            if !crate::shade::is_active() {
                crate::shade::init();
            }
            crate::shade::with_compositor(|comp| {
                comp.create_window("loop", 0, 0, 800, 600);
                if let Some(id2) = comp.create_window("editor", 0, 0, 800, 600) {
                    if let Some(win) = comp.window_mut(id2) {
                        win.bg_color = 0x00180820;
                    }
                }
                if let Some(id3) = comp.create_window("status", 0, 0, 800, 300) {
                    if let Some(win) = comp.window_mut(id3) {
                        win.bg_color = 0x00081820;
                    }
                }
            });
            crate::shade::render_frame();
            kprintln!("[npk] shade: demo mode (3 windows, master-stack layout)");
        }
        "stop" | "exit" => {
            if !crate::shade::is_active() {
                kprintln!("[npk] shade: not running");
                return;
            }
            crate::shade::stop();
            kprintln!("[npk] shade: stopped");
        }
        "ws" | "workspace" => {
            kprintln!("[npk] Usage: shade ws <1-4>");
        }
        sub if sub.starts_with("ws ") || sub.starts_with("workspace ") => {
            let num_str = sub.split_whitespace().nth(1).unwrap_or("");
            if let Ok(ws) = num_str.parse::<u8>() {
                if ws >= 1 && ws <= 4 {
                    crate::shade::with_compositor(|comp| {
                        comp.switch_workspace(ws - 1);
                    });
                    crate::shade::render_frame();
                    kprintln!("[npk] shade: workspace {}", ws);
                } else {
                    kprintln!("[npk] shade: workspace 1-4");
                }
            }
        }
        "config" => {
            kprintln!();
            kprintln!("  Shade Compositor");
            kprintln!("  ────────────────");
            for (key, default, desc) in crate::shade::default_config() {
                let current = crate::config::get(key);
                let val = current.as_deref().unwrap_or(default);
                kprintln!("  {:24} = {:8}  {}", key, val, desc);
            }
            kprintln!();
            kprintln!("  Use 'set <key> <value>' to change.");
            kprintln!();
        }
        "status" | "" => {
            if crate::shade::is_active() {
                crate::shade::with_compositor(|comp| {
                    kprintln!("  shade: active");
                    kprintln!("  screen: {}x{} scale:{}x", comp.screen_w, comp.screen_h, comp.scale);
                    kprintln!("  windows: {}", comp.window_count());
                    kprintln!("  workspace: {}/4", comp.active_workspace + 1);
                    kprintln!("  gaps: {}px  border: {}px", comp.gaps, comp.border);
                    match comp.top_strut {
                        Some(s) => kprintln!("  bar: strut {}px (+{}px margin)", s.pill_h, s.margin),
                        None => kprintln!("  bar: none"),
                    }
                });
            } else {
                kprintln!("[npk] shade: not running (use 'shade init' to start)");
            }
        }
        _ => {
            kprintln!("Usage: shade [init|demo|stop|status|config|ws <1-4>]");
        }
    }
}

pub fn intent_dmesg() {
    // Stop capture, print, restart — so dmesg output itself isn't appended
    let log = crate::serial::stop_capture();
    if log.is_empty() {
        kprintln!("(no boot log captured)");
    } else {
        // Print without going through capture (direct serial + framebuffer)
        kprintln!("{}", log);
    }
    crate::serial::start_capture();
}

pub fn intent_uname(args: &str) {
    let all = args.contains("-a") || args.is_empty();
    if all {
        kprintln!("nopeekOS {} x86_64 release (rustc {})",
            env!("CARGO_PKG_VERSION"),
            rustc_version());
    } else {
        if args.contains("-s") { kprintln!("nopeekOS"); }
        if args.contains("-r") || args.contains("-v") {
            kprintln!("{}", env!("CARGO_PKG_VERSION"));
        }
        if args.contains("-m") { kprintln!("x86_64"); }
    }
}

fn rustc_version() -> &'static str {
    // Embedded at compile time via env
    option_env!("RUSTC_VERSION").unwrap_or("nightly")
}

pub fn intent_caps(vault: &Vault) {
    let (active, max) = vault.stats();
    kprintln!();
    kprintln!("  Capability Vault");
    kprintln!("  ────────────────");
    kprintln!("  Active tokens:  {}", active);
    kprintln!("  Max capacity:   {}", max);
    kprintln!("  Token IDs:      256-bit random (CSPRNG)");
    kprintln!();
    kprintln!("  Security model: Deny by Default");
    kprintln!("  No ambient authority. No root user. No sudo.");
    kprintln!("  Every action requires an explicit capability token.");
    kprintln!();
}

pub fn intent_audit() {
    use crate::audit::{self, AuditOp};

    let entries = audit::recent(10);
    let total = audit::total_count();

    kprintln!();
    kprintln!("  Audit Log ({} total events, showing last {})", total, entries.len());
    kprintln!("  ─────────────────────────────────────────────");

    if entries.is_empty() {
        kprintln!("  (no events recorded)");
    } else {
        for entry in &entries {
            let secs = entry.tick / 100;
            let ms = (entry.tick % 100) * 10;
            match entry.op {
                AuditOp::Create { parent_id, new_id } =>
                    kprintln!("  [{:>4}.{:03}s] CREATE {:08x} from {:08x}",
                        secs, ms, capability::short_id(&new_id), capability::short_id(&parent_id)),
                AuditOp::Revoke { revoker_id, target_id } =>
                    kprintln!("  [{:>4}.{:03}s] REVOKE {:08x} by {:08x}",
                        secs, ms, capability::short_id(&target_id), capability::short_id(&revoker_id)),
                AuditOp::Check { cap_id } =>
                    kprintln!("  [{:>4}.{:03}s] CHECK  {:08x} OK",
                        secs, ms, capability::short_id(&cap_id)),
                AuditOp::Denied { reason } =>
                    kprintln!("  [{:>4}.{:03}s] DENIED {:?}",
                        secs, ms, reason),
                AuditOp::Expired { cap_id } =>
                    kprintln!("  [{:>4}.{:03}s] EXPIRED {:08x}",
                        secs, ms, capability::short_id(&cap_id)),
            }
        }
    }
    kprintln!();
}

pub fn intent_time() {
    if crate::net::ntp::unix_time().is_none() {
        kprintln!("[npk] Syncing time...");
        crate::net::ntp::sync_via_dns("pool.ntp.org");
    }
    match crate::net::ntp::unix_time() {
        Some(t) => kprintln!("{}", crate::net::ntp::format_time(t)),
        None => kprintln!("[npk] Time unavailable. No RTC or network."),
    }
}

/// One command and what it does. `*` makes it a status line, so the terminal
/// colours the marker and steps `(…)` asides back — see `shade::terminal`.
fn help_row(cmd: &str, what: &str) {
    kprintln!("[npk]   * {:<21} {}", cmd, what);
}

/// One group in the overview: a short label, then the commands it holds.
fn help_group(label: &str, cmds: &str) {
    kprintln!("[npk]   * {:<10} {}", label, cmds);
}

/// A dimmed aside: subtitles, hints, "see also". `.` dims the whole line.
fn help_note(text: &str) {
    kprintln!("[npk]   . {}", text);
}

fn help_head(title: &str, subtitle: &str) {
    kprintln!("[npk] {}", title);
    if !subtitle.is_empty() { help_note(subtitle); }
    kprintln!("[npk]");
}

/// Help. ASCII only, on purpose: the terminal font draws 0x20..0x7E and
/// silently SKIPS anything above it, while the column arithmetic still counts
/// the bytes. The old help was full of `─`, `·` and `✓` — 297 of them — so
/// every rule appeared as a blank line and every separator as a gap. It only
/// ever looked right on the serial console.
pub fn intent_help_topic(topic: &str) {
    match topic {
        "files" | "storage" | "store" | "fs" | "content" | "cat" | "grep" => {
            help_head("files", "objects in a content-addressed store; paths are names for them");
            help_row("ls [path]", "List a directory");
            help_row("cd <path>", "Change the working directory");
            help_row("pwd", "Print the working directory");
            help_row("mkdir <path>", "Create a directory");
            help_row("rm <name>", "Remove an object");
            help_row("store <name> <data>", "Save an object");
            help_row("fetch <name>", "Load and print an object");
            help_row("cat <name>", "Print an object");
            help_row("head <name> [n]", "First n lines (default 10)");
            help_row("grep <pat> <name>", "Search lines, case-insensitive");
            help_row("wc <name>", "Count lines, words, bytes");
            help_row("hexdump <name> [n]", "Hex dump (default 256 bytes)");
            help_row("find <pattern>", "Search object names");
            help_row("list", "Every object with its hash");
            help_row("fsinfo", "Disk usage and block stats");
            help_row("gc", "Reclaim unreachable objects");
            kprintln!("[npk]");
            help_note("redirect with '>':  cat mypage > copy");
        }
        "net" | "network" | "http" | "https" => {
            help_head("net", "");
            help_row("ping <host>", "ICMP echo (IP or hostname)");
            help_row("resolve <host>", "DNS lookup");
            help_row("https <host> [path]", "HTTPS GET (TLS 1.3)");
            help_row("http <host> [path]", "HTTP GET (plaintext)");
            help_row("curl <host> [path]", "Alias of http (also: wget)");
            help_row("netstat", "Active connections");
            help_row("net", "Interface state");
            help_row("ifconfig", "Address, gateway, MAC");
            help_row("dhcp", "Renew the lease");
            help_row("dns <host>", "Resolver detail");
            help_row("traceroute <host>", "Path trace");
            help_row("nic", "USB-NIC scan (link + speed)");
            help_row("netbench", "Throughput measurement");
            kprintln!("[npk]");
            help_note("save a download:  https example.com /page.html > page");
        }
        "packages" | "install" | "modules" | "update" | "assets" => {
            help_head("packages", "signed (ECDSA P-384) and verified on every path");
            help_row("modules", "Installed WASM modules");
            help_row("assets", "Fonts, icons, microvm payloads, wallpapers");
            help_row("install <module>", "Download, verify, install");
            help_row("uninstall <module>", "Remove (--force for bundled)");
            help_row("update", "Show what is new, then ask");
            help_row("update -y", "Apply without asking");
            help_row("update -v", "Keep the connect/HTTP diagnostics");
            kprintln!("[npk]");
            help_note("source: raw.githubusercontent.com/fnopeek/nopeekOS");
        }
        "apps" | "exec" | "wasm" | "run" | "browser" => {
            help_head("apps", "");
            help_row("run <module> [args]", "Execute a WASM module");
            help_row("driver <name>", "Load a WASM hardware driver");
            help_row("browser", "LibreWolf in a tiled MicroVM");
            kprintln!("[npk]");
            help_note("the dock and Mod+D start the same modules; 'modules' lists them");
        }
        "desktop" | "shade" | "compositor" | "wm" | "display" | "wallpaper" | "wp" => {
            help_head("desktop", "shade: the tiling compositor");
            help_row("shade", "Compositor state (also: shade config)");
            help_row("shade ws <1-4>", "Switch workspace");
            help_row("wallpaper set <name>", "Set wallpaper (also: list, random, clear)");
            help_row("theme", "Palette state (from the wallpaper)");
            help_row("dark | light | auto", "Switch the theme mode");
            help_row("gpu", "Graphics adapter and mode");
            help_row("volume [0-100]", "Audio level");
            help_row("battery", "Charge level");
            help_row("mouse speed <25-600>", "Pointer speed (also: mouse size)");
            kprintln!("[npk]");
            help_note("keys: Mod+Enter new  Mod+Q close  Mod+D launcher  Mod+F fullscreen");
            help_note("      Mod+J flip split  Mod+arrow focus  Mod+Shift+arrow move");
            help_note("      Mod+Ctrl+arrow resize  Mod+1-4 workspace  Mod+Shift+1-4 send");
            kprintln!("[npk]");
            help_note("config: shade.gaps, shade.border, shade.rounding, shade.glow,");
            help_note("        shade.opacity, shade.mod");
        }
        "system" | "status" => {
            help_head("system", "");
            help_row("status", "Kernel, memory, uptime at a glance");
            help_row("top", "Live processes and cores");
            help_row("uptime", "Time since boot");
            help_row("time", "Clock (also: date)");
            help_row("dmesg", "Kernel log");
            help_row("bootlog", "Log of the current boot");
            help_row("cores", "Per-core load and frequency");
            help_row("cpu", "CPU model and features");
            help_row("slab", "Allocator statistics");
            help_row("history", "Previous intents");
            help_row("clear", "Clear the window");
            help_row("version", "Kernel version (also: uname, about)");
            help_row("reboot", "Restart");
            help_row("halt", "Power off");
            help_row("exit", "Close this loop window");
            kprintln!("[npk]");
            help_row("echo <text>", "Print text");
            help_row("about", "What this system is (also: philosophy)");
        }
        "security" | "lock" | "caps" => {
            help_head("security", "capabilities, not permissions: no chmod, no ACLs, no root");
            help_row("lock", "Lock the system (clears keys)");
            help_row("passwd", "Change the passphrase");
            help_row("caps", "Capability vault");
            help_row("audit", "Security event log");
            help_row("shell", "Encrypted remote shell (port 4444)");
        }
        "config" | "set" | "settings" => {
            help_head("config", "");
            help_row("config", "Every setting");
            help_row("set <key> <value>", "Change one");
            help_row("get <key>", "Read one");
            kprintln!("[npk]");
            help_note("keys: name, timezone, keyboard, lang, accent, theme, launcher,");
            help_note("      autostart, mouse_speed, shade.* (see 'help desktop')");
        }
        "hardware" | "hw" | "disk" | "blk" | "pci" | "usb" => {
            help_head("hardware", "");
            help_row("pci", "PCI devices (also: lspci)");
            help_row("usb", "USB devices (also: lsusb)");
            help_row("xhci", "USB controllers and port link state");
            help_row("disk", "Disk info");
            help_row("disk read <sector>", "Raw sector hex dump");
            help_row("disk write <s> <txt>", "Write text to a sector");
            help_row("fbinfo", "Framebuffer geometry");
            help_row("beep", "Speaker test");
            help_row("test-audio", "Audio output test");
        }
        "virt" | "microvm" | "vmx" | "vt-x" => {
            help_head("virt", "Linux apps inside a hardware-isolated guest");
            help_row("microvm test", "Substrate smoke test");
            help_row("microvm linux-info", "Parse the bundled bzImage");
            help_row("microvm linux", "Boot the bundled userspace");
            help_row("vmx", "Probe Intel VT-x support");
            kprintln!("[npk]");
            help_note("'browser' is the app-facing way in; this is the substrate");
            help_note("no VT-x? enable Intel Virtualization Technology in UEFI setup");
        }
        _ => {
            kprintln!("[npk] nopeekOS v{}", env!("CARGO_PKG_VERSION"));
            kprintln!("[npk]");
            help_group("files", "ls  cd  cat  find  store  fetch  rm  fsinfo");
            help_group("net", "ping  resolve  https  netstat  nic  dhcp");
            help_group("packages", "modules  assets  install  update");
            help_group("apps", "run  driver  browser");
            help_group("desktop", "shade  wallpaper  theme  gpu  volume  battery");
            help_group("system", "status  top  dmesg  cores  reboot  halt");
            help_group("security", "lock  passwd  caps  audit  shell");
            help_group("config", "config  set  get");
            help_group("hardware", "pci  usb  disk  fbinfo");
            help_group("virt", "microvm  vmx");
            kprintln!("[npk]");
            help_note("help <topic> for any of those");
        }
    }
}

pub fn intent_about() {
    kprintln!();
    kprintln!("  nopeekOS – AI-native Operating System");
    kprintln!("  ──────────────────────────────────────");
    kprintln!();
    kprintln!("  Not a Unix clone. Not POSIX. No legacy.");
    kprintln!("  Built for AI as the operator, humans as the conductor.");
    kprintln!();
    kprintln!("  Capabilities, not permissions. Intents, not commands.");
    kprintln!("  Content-addressed, not paths. Runtime-generated, not installed.");
    kprintln!();
    kprintln!("  Created in Luzern, Switzerland.");
    kprintln!();
}

pub fn intent_philosophy() {
    kprintln!();
    kprintln!("  What remains when you remove fifty years of assumptions?");
    kprintln!();
    kprintln!("  A capability vault, a WASM sandbox,");
    kprintln!("  an intent loop, and a human view.");
    kprintln!("  Everything else is generated.");
    kprintln!();
}

pub fn intent_echo(args: &str) { kprintln!("{}", args); }

pub fn intent_think(args: &str) {
    kprintln!();
    kprintln!("  [Intent: think]");
    kprintln!("  Question: {}", args);
    kprintln!();
    kprintln!("  AI reasoning not yet available.");
    kprintln!("  This will route to the neurofabric layer (Phase 7+).");
    kprintln!();
}

pub fn intent_set(args: &str) {
    let args = args.trim();
    if let Some((key, value)) = args.split_once(' ') {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            kprintln!("[npk] Usage: set <key> <value>");
            return;
        }
        crate::config::set(key, value);
        kprintln!("[npk] {} = {}", key, value);
        // Live-apply rendering keys read fresh each frame (e.g.
        // shade.light_tint) so tuning shows at once, not on the next
        // incidental redraw. Struct-cached keys (opacity) still need a
        // compositor rebuild — unchanged.
        if key.starts_with("shade.") || key == "theme" || key == "accent" {
            crate::shade::force_redraw();
        }
    } else {
        kprintln!("[npk] Usage: set <key> <value>");
        kprintln!("[npk] Keys: timezone, keyboard, lang");
        kprintln!("[npk] Example: set timezone +2");
    }
}

pub fn intent_get(args: &str) {
    let key = args.trim();
    if key.is_empty() {
        kprintln!("[npk] Usage: get <key>");
        return;
    }
    match crate::config::get(key) {
        Some(val) => kprintln!("{} = {}", key, val),
        None => kprintln!("[npk] '{}' not set", key),
    }
}

pub fn intent_config() {
    let entries = crate::config::list();
    if entries.is_empty() {
        kprintln!("[npk] No configuration set.");
        kprintln!("[npk] Use 'set <key> <value>' to configure.");
    } else {
        kprintln!();
        for (k, v) in &entries {
            kprintln!("  {} = {}", k, v);
        }
        kprintln!();
    }
    kprintln!("[npk] Available keys:");
    for (key, desc) in crate::config::KNOWN_KEYS {
        kprintln!("  {:12} {}", key, desc);
    }
}

pub fn intent_reboot() -> ! {
    kprintln!();
    kprintln!("[npk] Rebooting...");
    kprintln!();
    unsafe {
        // Disable interrupts first
        core::arch::asm!("cli");

        // Method 1: ACPI reset register (if available from FADT)
        crate::acpi::reset();

        // Method 2: PCI CF9 reset (Intel chipsets)
        // Must write 0x02 first (enable reset), then 0x06 (trigger)
        core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x02u8);
        for _ in 0..100_000u32 { core::hint::spin_loop(); }
        core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x06u8);
        for _ in 0..1_000_000u32 { core::hint::spin_loop(); }

        // Method 3: Keyboard controller reset (port 0x64)
        core::arch::asm!("out dx, al", in("dx") 0x64u16, in("al") 0xFEu8);
        for _ in 0..1_000_000u32 { core::hint::spin_loop(); }

        // Method 4: Triple-fault (guaranteed reboot on any x86)
        let null_idt: [u8; 6] = [0; 6];
        core::arch::asm!("lidt [{}]", in(reg) &null_idt);
        core::arch::asm!("int3");

        loop { core::arch::asm!("hlt"); }
    }
}

pub fn intent_lspci(args: &str) {
    use crate::drivers::pci::{self, PciAddr};

    let verbose = args.contains("-v");
    let mut count = 0u16;

    kprintln!();
    for bus in 0u16..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let addr = PciAddr { bus: bus as u8, device: dev, function: func };
                let id = pci::read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 {
                    if func == 0 { break; }
                    continue;
                }

                let vid = (id & 0xFFFF) as u16;
                let did = ((id >> 16) & 0xFFFF) as u16;
                let class_reg = pci::read32(addr, 0x08);
                let cls = ((class_reg >> 24) & 0xFF) as u8;
                let sub = ((class_reg >> 16) & 0xFF) as u8;
                let prog_if = ((class_reg >> 8) & 0xFF) as u8;
                let rev = (class_reg & 0xFF) as u8;

                let class_name = pci_class_name(cls, sub, prog_if);
                let dev_name = pci_device_name(vid, did);

                kprintln!("  {:02x}:{:02x}.{}  {:04x}:{:04x}  {}",
                    bus, dev, func, vid, did, class_name);
                if !dev_name.is_empty() {
                    kprintln!("           {}", dev_name);
                }

                if verbose {
                    let bar0 = pci::read32(addr, 0x10);
                    let irq = pci::read8(addr, 0x3C);
                    let cmd = pci::read16(addr, 0x04);
                    kprintln!("           rev {:02x}  prog-if {:02x}  IRQ {}  BAR0 {:08x}",
                        rev, prog_if, irq, bar0);
                    kprintln!("           cmd: {}{}{}",
                        if cmd & 0x04 != 0 { "bus-master " } else { "" },
                        if cmd & 0x02 != 0 { "mem " } else { "" },
                        if cmd & 0x01 != 0 { "io" } else { "" });
                }

                count += 1;

                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 {
                    break;
                }
            }
        }
    }
    kprintln!();
    kprintln!("  {} PCI devices found", count);
    kprintln!();
}

/// `mouse [speed <n>] [size <n>]` — show or set pointer speed (25..=600 %) and
/// cursor size (50..=300 %). Persisted to `mouse_speed` / `mouse_size` config.
pub fn intent_mouse(args: &str) {
    let a = args.trim();
    if let Some(v) = a.strip_prefix("speed") {
        let v = v.trim();
        if v.is_empty() {
            kprintln!("  mouse speed: {}%", crate::shade::cursor::speed());
        } else if let Ok(n) = v.parse::<i32>() {
            crate::shade::cursor::set_speed(n);
            let now = crate::shade::cursor::speed();
            crate::config::set("mouse_speed", &alloc::format!("{}", now));
            kprintln!("  mouse speed set to {}%", now);
        } else {
            kprintln!("  usage: mouse speed <25-600>");
        }
    } else if let Some(v) = a.strip_prefix("size") {
        let v = v.trim();
        if v.is_empty() {
            kprintln!("  mouse size: {}%", crate::shade::cursor::size());
        } else if let Ok(n) = v.parse::<i32>() {
            crate::shade::cursor::set_size(n);
            let now = crate::shade::cursor::size();
            crate::config::set("mouse_size", &alloc::format!("{}", now));
            kprintln!("  mouse size set to {}%", now);
        } else {
            kprintln!("  usage: mouse size <50-300>");
        }
    } else if a.is_empty() {
        kprintln!("  mouse speed {}%  size {}%", crate::shade::cursor::speed(), crate::shade::cursor::size());
        kprintln!("  set: mouse speed <25-600> | mouse size <50-300>");
    } else {
        kprintln!("  usage: mouse [speed <25-600>] [size <50-300>]");
    }
}

/// `usb` / `lsusb` — enumerate every device on every xHCI controller and
/// print VID:PID + class + product. Identifies dongles (NICs etc.) so the
/// driver catalog can match the right WASM driver.
pub fn intent_lsusb() {
    kprintln!();
    kprintln!("  Enumerating USB (re-plug a USB keyboard/mouse afterwards)...");
    crate::xhci::list_devices();
}

fn pci_class_name(cls: u8, sub: u8, prog_if: u8) -> &'static str {
    match (cls, sub, prog_if) {
        (0x00, 0x00, _) => "Legacy device",
        (0x00, 0x01, _) => "VGA-compatible",
        (0x01, 0x00, _) => "SCSI controller",
        (0x01, 0x01, _) => "IDE controller",
        (0x01, 0x06, _) => "SATA controller",
        (0x01, 0x08, 0x02) => "NVMe controller",
        (0x01, 0x08, _) => "NVM controller",
        (0x02, 0x00, _) => "Ethernet controller",
        (0x02, 0x80, _) => "Network controller",
        (0x03, 0x00, _) => "VGA controller",
        (0x03, 0x80, _) => "Display controller",
        (0x04, 0x00, _) => "Video controller",
        (0x04, 0x01, _) => "Audio controller",
        (0x04, 0x03, _) => "HD Audio controller",
        (0x06, 0x00, _) => "Host bridge",
        (0x06, 0x01, _) => "ISA bridge",
        (0x06, 0x04, _) => "PCI-to-PCI bridge",
        (0x06, 0x80, _) => "System bridge",
        (0x07, 0x00, _) => "Serial controller",
        (0x07, 0x80, _) => "Communication controller",
        (0x08, 0x00, _) => "PIC",
        (0x08, 0x01, _) => "DMA controller",
        (0x08, 0x02, _) => "Timer",
        (0x08, 0x03, _) => "RTC controller",
        (0x08, 0x80, _) => "System peripheral",
        (0x0C, 0x03, 0x00) => "UHCI USB controller",
        (0x0C, 0x03, 0x10) => "OHCI USB controller",
        (0x0C, 0x03, 0x20) => "EHCI USB controller",
        (0x0C, 0x03, 0x30) => "xHCI USB controller",
        (0x0C, 0x03, _) => "USB controller",
        (0x0C, 0x05, _) => "SMBus controller",
        (0x0D, 0x00, _) => "IrDA controller",
        (0x0D, 0x80, _) => "Wireless controller",
        (0x0E, 0x00, _) => "I2O controller",
        (0x0F, _, _) => "Satellite controller",
        (0x10, _, _) => "Crypto controller",
        (0x11, _, _) => "Signal processing",
        (0xFF, _, _) => "Unassigned",
        _ => "Unknown",
    }
}

fn pci_device_name(vendor: u16, device: u16) -> &'static str {
    match (vendor, device) {
        // Intel WiFi
        (0x8086, 0x2723) => "Intel Wi-Fi 6 AX200",
        (0x8086, 0x2725) => "Intel Wi-Fi 6E AX210",
        (0x8086, 0x4DF0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0xA0F0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0x06F0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0x34F0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0x51F0) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x51F1) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x54F0) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x7AF0) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x7E40) => "Intel Wi-Fi 7 BE200",
        (0x8086, 0xE440) => "Intel Wi-Fi 7 BE200",
        (0x8086, 0x272B) => "Intel Wi-Fi 7 BE202",
        // Intel Ethernet
        (0x8086, 0x15F3) => "Intel I225-V (2.5GbE)",
        (0x8086, 0x15F2) => "Intel I225-LM (2.5GbE)",
        (0x8086, 0x125C) => "Intel I226-V (2.5GbE)",
        (0x8086, 0x125B) => "Intel I226-LM (2.5GbE)",
        (0x8086, 0x15E3) => "Intel I219-LM",
        (0x8086, 0x0D4F) => "Intel I219-V",
        (0x8086, 0x15BE) => "Intel I219-LM",
        (0x8086, 0x15BD) => "Intel I219-V",
        // Intel GPU
        (0x8086, 0x46A6) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0x46D0) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0x46D1) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0x46D2) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0xA7A0) => "Intel Raptor Lake [UHD Graphics]",
        (0x8086, 0xA720) => "Intel Raptor Lake [UHD Graphics]",
        (0x8086, 0xA780) => "Intel Raptor Lake [UHD Graphics]",
        (0x8086, 0x4628) => "Intel Alder Lake [Iris Xe]",
        (0x8086, 0x4626) => "Intel Alder Lake [Iris Xe]",
        (0x8086, 0x46A8) => "Intel Alder Lake [Iris Xe]",
        // Intel NVMe
        (0x8086, 0xF1A8) => "Intel SSD 660p/670p",
        (0x8086, 0xF1AA) => "Intel SSD 670p",
        // Intel Host Bridge / ISA / misc
        (0x8086, 0x4617) => "Intel Alder Lake Host Bridge",
        (0x8086, 0x461C) => "Intel Alder Lake-N Host Bridge",
        (0x8086, 0x4601) => "Intel Alder Lake Host Bridge",
        (0x8086, 0x461D) => "Intel Alder Lake-N TurboBoost",
        (0x8086, 0x467E) => "Intel Alder Lake-N GNA",
        (0x8086, 0x467D) => "Intel Alder Lake-N IPU",
        (0x8086, 0x4649) => "Intel Alder Lake PCIe RP",
        (0x8086, 0x464D) => "Intel Alder Lake PCIe RP",
        (0x8086, 0x4641) => "Intel Alder Lake PCH",
        (0x8086, 0x5481) => "Intel Alder Lake-N ISA Bridge",
        (0x8086, 0x51A3) => "Intel Alder Lake-P ISA Bridge",
        (0x8086, 0x54A3) => "Intel Alder Lake-N SMBus",
        (0x8086, 0x51EF) => "Intel Alder Lake-P SMBus",
        (0x8086, 0x54A4) => "Intel Alder Lake-N SPI Controller",
        (0x8086, 0x54C4) => "Intel Alder Lake-N eSPI/SPI",
        (0x8086, 0x54EF) => "Intel Alder Lake-N Shared SRAM",
        (0x8086, 0x54E8) => "Intel Alder Lake-N Serial IO I2C #0",
        (0x8086, 0x54EA) => "Intel Alder Lake-N Serial IO I2C #2",
        (0x8086, 0x54EB) => "Intel Alder Lake-N Serial IO I2C #3",
        (0x8086, 0x54E0) => "Intel Alder Lake-N HECI/MEI",
        (0x8086, 0x54D3) => "Intel Alder Lake-N SATA AHCI",
        (0x8086, 0x51E8) => "Intel Alder Lake-P Serial IO I2C",
        // Intel HD Audio
        (0x8086, 0x54C8) => "Intel Alder Lake-N HD Audio",
        (0x8086, 0x51C8) => "Intel Alder Lake-P HD Audio",
        (0x8086, 0x51CA) => "Intel Alder Lake-P HD Audio",
        (0x8086, 0x4DC8) => "Intel Alder Lake-N HD Audio",
        // Intel PCI-to-PCI bridges (Alder Lake-N)
        (0x8086, 0x54BE) => "Intel Alder Lake-N PCIe RP #7",
        (0x8086, 0x54B0) => "Intel Alder Lake-N PCIe RP #9",
        (0x8086, 0x54B2) => "Intel Alder Lake-N PCIe RP #11",
        // Intel Thunderbolt / USB
        (0x8086, 0x461E) => "Intel Alder Lake Thunderbolt 4",
        (0x8086, 0x464E) => "Intel Alder Lake-N xHCI",
        (0x8086, 0x54ED) => "Intel Alder Lake-N PCH xHCI",
        (0x8086, 0x51ED) => "Intel Alder Lake-P xHCI",
        (0x8086, 0x4DED) => "Intel Alder Lake-N xHCI",
        // Samsung NVMe
        (0x144D, 0xA808) => "Samsung 970 EVO Plus",
        (0x144D, 0xA809) => "Samsung 980 PRO",
        (0x144D, 0xA80A) => "Samsung 990 PRO",
        // Virtio (QEMU)
        (0x1AF4, 0x1000) => "VirtIO Network (legacy)",
        (0x1AF4, 0x1041) => "VirtIO Network",
        (0x1AF4, 0x1001) => "VirtIO Block (legacy)",
        (0x1AF4, 0x1042) => "VirtIO Block",
        (0x1AF4, 0x1050) => "VirtIO GPU",
        // Realtek
        (0x10EC, 0x8168) => "Realtek RTL8111/8168",
        (0x10EC, 0x8125) => "Realtek RTL8125 (2.5GbE)",
        (0x10EC, 0xB852) => "Realtek RTL8852BE (Wi-Fi 6)",
        (0x10EC, 0xB832) => "Realtek RTL8832BE (Wi-Fi 6E)",
        (0x10EC, 0xC852) => "Realtek RTL8852CE (Wi-Fi 6E)",
        // MAXIO NVMe
        (0x1E4B, 0x1202) => "MAXIO MAP1202 NVMe SSD",
        (0x1E4B, 0x1602) => "MAXIO MAP1602 NVMe SSD",
        // QEMU/VBox
        (0x8086, 0x100E) => "Intel 82540EM (QEMU e1000)",
        (0x8086, 0x29C0) => "Intel 82G33 Host Bridge (QEMU)",
        (0x8086, 0x2918) => "Intel ICH9 LPC (QEMU)",
        (0x8086, 0x2922) => "Intel ICH9 AHCI (QEMU)",
        (0x8086, 0x2930) => "Intel ICH9 SMBus (QEMU)",
        (0x1234, 0x1111) => "QEMU/Bochs VGA",
        _ => "",
    }
}

pub fn intent_halt() -> ! {
    kprintln!();
    kprintln!("[npk] Shutting down...");
    kprintln!("[npk] Goodbye.");
    kprintln!();
    unsafe {
        // Try QEMU exit (harmless on real hardware)
        core::arch::asm!("out dx, al", in("dx") 0xf4u16, in("al") 0u8);

        // ACPI S5 power-off (port discovered from FADT at boot)
        crate::acpi::power_off();

        // Fallback: hardcoded common PM1a_CNT ports
        let slp_s5: u16 = (5 << 10) | (1 << 13);
        core::arch::asm!("out dx, ax", in("dx") 0x604u16, in("ax") slp_s5);
        core::arch::asm!("out dx, ax", in("dx") 0x1804u16, in("ax") slp_s5);

        // Last resort: triple-fault reboot
        let null_idt: [u8; 6] = [0; 6];
        core::arch::asm!("lidt [{}]", in(reg) &null_idt);
        core::arch::asm!("int3");

        loop { core::arch::asm!("cli; hlt"); }
    }
}
