//! The Raspberry Pi 5's SD card slot: the BCM2712 SD controller from its
//! device tree node, with the board's supplies as GPIO lines (Linux
//! `regulator-fixed` for card power, `regulator-gpio` for the signal
//! voltage, `cd-gpios`), then the card through `mmc_sd`.

use super::dt::{Fdt, Node};
use super::gpio::Line;
use crate::drivers::{mmc_sd, sdhci};
use crate::hw::Mmio;
use crate::kprintln;

/// Card power: the enable line of the fixed 3.3 V regulator.
static VMMC: spin::Mutex<Option<Line>> = spin::Mutex::new(None);
/// Signal voltage: the line, its value for 1.8 V and for 3.3 V, the
/// settling time in microseconds.
static VQMMC: spin::Mutex<Option<(Line, bool, bool, u64)>> = spin::Mutex::new(None);
static CD: spin::Mutex<Option<Line>> = spin::Mutex::new(None);

fn vmmc(on: bool) {
    if let Some(l) = *VMMC.lock() {
        l.set(on);
    }
}

/// `regulator_set_voltage` on the GPIO regulator, then its settling time.
fn vqmmc(v18: bool) {
    let Some((l, at18, at33, settle_us)) = *VQMMC.lock() else { return };
    l.set(if v18 { at18 } else { at33 });
    crate::time::delay_us(settle_us);
}

fn cd() -> bool {
    CD.lock().map(|l| l.get()).unwrap_or(true)
}

/// The rate of the node's first clock, where it is a `fixed-clock`.
fn clock_rate(fdt: &Fdt, n: &Node) -> Option<u32> {
    let c = fdt.find_phandle(fdt.prop_cell(n, b"clocks", 0)?)?;
    if !fdt.prop_has_str(&c, b"compatible", b"fixed-clock") { return None; }
    fdt.prop_u32(&c, b"clock-frequency")
}

/// The `reg` entry named `name` in `reg-names`.
fn reg_named(fdt: &Fdt, n: &Node, name: &[u8]) -> Option<(u64, u64)> {
    let idx = fdt.prop_str_index(n, b"reg-names", name)?;
    let mut r = None;
    let mut i = 0;
    fdt.reg(n, |a, s| {
        if i == idx { r = Some((a, s)); }
        i += 1;
    });
    r
}

/// `mmc_of_parse`: the capabilities the node adds.
fn dt_caps(fdt: &Fdt, n: &Node) -> u32 {
    let mut caps = 0;
    match fdt.prop_u32(n, b"bus-width").unwrap_or(1) {
        8 => caps |= sdhci::MMC_CAP_8_BIT_DATA | sdhci::MMC_CAP_4_BIT_DATA,
        4 => caps |= sdhci::MMC_CAP_4_BIT_DATA,
        _ => {}
    }
    let has = |p: &[u8]| fdt.prop(n, p).is_some();
    if has(b"sd-uhs-sdr12") { caps |= sdhci::MMC_CAP_UHS_SDR12; }
    if has(b"sd-uhs-sdr25") { caps |= sdhci::MMC_CAP_UHS_SDR25; }
    if has(b"sd-uhs-sdr50") { caps |= sdhci::MMC_CAP_UHS_SDR50; }
    if has(b"sd-uhs-sdr104") { caps |= sdhci::MMC_CAP_UHS_SDR104; }
    if has(b"sd-uhs-ddr50") { caps |= sdhci::MMC_CAP_UHS_DDR50; }
    if has(b"non-removable") { caps |= sdhci::MMC_CAP_NONREMOVABLE; }
    caps
}

/// A `regulator-fixed` with an enable GPIO: its line, enabled as the
/// firmware left it (`regulator-boot-on`).
fn fixed_regulator(fdt: &Fdt, r: &Node) -> Option<Line> {
    if !fdt.prop_has_str(r, b"compatible", b"regulator-fixed") { return None; }
    let l = Line::from_dt(fdt, r, b"gpios", 0)?;
    l.output(fdt.prop(r, b"regulator-boot-on").is_some());
    Some(l)
}

/// A `regulator-gpio` with one line: the line, its value at 1.8 V and at
/// 3.3 V, the settling time; left at the state `gpios-states` gives
/// (`gpio_regulator_probe`), 0 without one.
fn gpio_regulator(fdt: &Fdt, r: &Node) -> Option<(Line, bool, bool, u64)> {
    if !fdt.prop_has_str(r, b"compatible", b"regulator-gpio") { return None; }
    let l = Line::from_dt(fdt, r, b"gpios", 0)?;
    let (mut at18, mut at33) = (None, None);
    let mut i = 0;
    while let (Some(uv), Some(v)) = (fdt.prop_cell(r, b"states", i), fdt.prop_cell(r, b"states", i + 1)) {
        if (1_700_000..=1_950_000).contains(&uv) { at18 = Some(v != 0); }
        if (2_700_000..=3_600_000).contains(&uv) { at33 = Some(v != 0); }
        i += 2;
    }
    let settle = fdt.prop_u32(r, b"regulator-settling-time-us").unwrap_or(0) as u64;
    l.output(fdt.prop_cell(r, b"gpios-states", 0).unwrap_or(0) != 0);
    Some((l, at18?, at33?, settle))
}

/// Find the SD controller, bring it up and initialise the card. The disk's
/// id when a card answered.
pub fn init(fdt: &Fdt) -> Option<u32> {
    let n = fdt.find_compatible(b"brcm,bcm2712-sdhci")?;
    let (Some((hb, hl)), Some((cb, cl))) = (reg_named(fdt, &n, b"host"), reg_named(fdt, &n, b"cfg")) else {
        kprintln!("[npk] sd: node without host/cfg registers");
        return None;
    };
    // SAFETY: the two register windows of the SD controller node.
    let (Ok(host), Ok(cfg)) = (unsafe { Mmio::map(hb, hl) }, unsafe { Mmio::map(cb, cl) }) else {
        kprintln!("[npk] sd: register windows not mappable");
        return None;
    };
    let supply = |p: &[u8]| fdt.prop_u32(&n, p).and_then(|ph| fdt.find_phandle(ph));
    *VMMC.lock() = supply(b"vmmc-supply").and_then(|r| fixed_regulator(fdt, &r));
    *VQMMC.lock() = supply(b"vqmmc-supply").and_then(|r| gpio_regulator(fdt, &r));
    let cd_line = Line::from_dt(fdt, &n, b"cd-gpios", 0);
    if let Some(l) = cd_line { l.input(); }
    *CD.lock() = cd_line;
    let supplies = sdhci::Supplies {
        vmmc: VMMC.lock().map(|_| vmmc as fn(bool)),
        vqmmc: VQMMC.lock().map(|_| vqmmc as fn(bool)),
        cd: cd_line.map(|_| cd as fn() -> bool),
    };
    let p = sdhci::Bcm2712 {
        host, cfg,
        clk_hz: clock_rate(fdt, &n).unwrap_or(0),
        caps: dt_caps(fdt, &n),
        supplies,
    };
    let h = sdhci::probe_bcm2712(p)?;
    kprintln!("[npk] sd: card {}", if cd_line.is_none() || cd() { "present" } else { "absent" });
    mmc_sd::init_host(h);
    mmc_sd::disks().first().map(|d| d.id)
}

/// Read the partition table, then one block past the boot image: write a
/// pattern, read it back, put the old content back.
pub fn self_test(id: u32) {
    let mut mbr = [0u8; 512];
    if let Err(e) = mmc_sd::read(id, 0, &mut mbr) {
        kprintln!("[npk] sd: reading the MBR failed: {:?}", e);
        return;
    }
    let sig = u16::from_le_bytes([mbr[510], mbr[511]]);
    kprintln!("[npk] sd: MBR signature {:#06x}", sig);
    for i in 0..4 {
        let e = &mbr[446 + 16 * i..446 + 16 * (i + 1)];
        let ty = e[4];
        if ty == 0 { continue; }
        let lba = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        let n = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
        kprintln!("[npk] sd:   partition {}: type {:#04x}, LBA {}, {} MB", i + 1, ty, lba, n as u64 * 512 / 1_000_000);
    }
    // Past the image's 64 MiB, which ends the written part of the card.
    const LBA: u64 = 140_000;
    let mut old = [0u8; 512];
    let mut back = [0u8; 512];
    let mut pat = [0u8; 512];
    for (i, b) in pat.iter_mut().enumerate() { *b = (i as u8) ^ 0x5a; }
    let r = mmc_sd::read(id, LBA, &mut old)
        .and_then(|_| mmc_sd::write(id, LBA, &pat))
        .and_then(|_| mmc_sd::read(id, LBA, &mut back));
    let restored = mmc_sd::write(id, LBA, &old).and_then(|_| {
        let mut again = [0u8; 512];
        mmc_sd::read(id, LBA, &mut again).map(|_| again == old)
    });
    match r {
        Ok(()) => kprintln!("[npk] sd: write/read at LBA {}: {}, old content back: {}",
            LBA, if back == pat { "ok" } else { "MISMATCH" }, matches!(restored, Ok(true))),
        Err(e) => kprintln!("[npk] sd: write/read at LBA {} failed: {:?}", LBA, e),
    }
}
