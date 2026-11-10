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

/// The card as 512-byte sectors, for the partition table.
struct Card {
    id: u32,
    sectors: u64,
}

impl crate::gpt::Sectors for Card {
    fn read(&mut self, lba: u64, buf: &mut [u8; 512]) -> Result<(), ()> {
        mmc_sd::read(self.id, lba, buf).map_err(|_| ())
    }
    fn write(&mut self, lba: u64, buf: &[u8; 512]) -> Result<(), ()> {
        mmc_sd::write(self.id, lba, buf).map_err(|_| ())
    }
    fn capacity(&mut self) -> Option<u64> {
        Some(self.sectors)
    }
}

/// The system disk on the card: the image's npkFS partition grows over the
/// card on the first boot (`gpt::grow_last_partition`). The partition's
/// start and size in 4 KiB blocks.
pub fn system_disk(id: u32) -> Option<(u64, u64)> {
    let sectors = mmc_sd::disks().iter().find(|d| d.id == id)
        .map(|d| d.blocks * d.block_size as u64 / 512)?;
    let mut card = Card { id, sectors };
    match crate::gpt::grow_last_partition(&mut card) {
        Ok(crate::gpt::Grown::Grew { from, to }) =>
            kprintln!("[npk] sd: npkFS partition grown from {} MB to {} MB, over the whole card",
                from * 512 / 1_000_000, to * 512 / 1_000_000),
        Ok(crate::gpt::Grown::Already { sectors }) =>
            kprintln!("[npk] sd: npkFS partition spans the card ({} MB)", sectors * 512 / 1_000_000),
        Err(e) => {
            kprintln!("[npk] sd: partition table: {}", e);
            return None;
        }
    }
    let (off, blocks) = crate::gpt::npkfs_partition(&mut card)?;
    kprintln!("[npk] sd: npkFS partition at 4K block {}, {} blocks ({} MB)", off, blocks, blocks * 4096 / 1_000_000);
    Some((off, blocks))
}
