//! audio_acp: the digital microphone on AMD Renoir's audio coprocessor
//! (ACP 3.x, `1022:15e2` revision 1), after Linux 6.18.60
//! `sound/soc/amd/renoir`: `rn-pci-acp3x.c` (power, reset, interrupts,
//! the `_WOV` presence check) and `acp3x-pdm-dma.c` (the PDM ring, its
//! DMA through the ACP's address translation, start and stop).
//!
//! The microphone goes into the device register as an audio input; while
//! it is the active one and a capture stream is open (`capture_wanted`),
//! the PDM DMA runs and what it records goes to the kernel's capture
//! side. Otherwise the DMA is stopped. 48 kHz, S32LE, stereo, as the
//! hardware delivers it (`acp_pdm_hardware_capture`).
//!
//! Not ported: the DMI table of machines whose `_WOV` claims a microphone
//! that is not there (`rn_acp_quirk_table`), since nopeek reads no SMBIOS
//! yet; and runtime power management (the ACP stays powered while this
//! module runs, `acp_power_gating` is 0 by default in Linux too).

#![no_std]

extern crate alloc;

mod regs;

use aml_core::{Ec, Machine, Namespace, Value};
use npk_sys::bump::Bump;
use regs::*;

#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x40];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[audio_acp] panic\n");
    core::arch::wasm32::unreachable()
}

const HEAP_SIZE: usize = 8 * 1024 * 1024;
#[global_allocator]
static ALLOC: Bump<HEAP_SIZE> = Bump::new();

fn log(s: &str) {
    npk_sys::print(s.as_bytes());
}

/// `pdm_gain`, Linux' default.
const PDM_GAIN: u32 = 3;
/// `acp_pdm_hardware_capture`: S32LE, stereo, 48 kHz.
const RATE: i32 = 48_000;
const ENCODING_S32: i32 = 1;
const CHANNELS: i32 = 2;
/// The ring Linux' ALSA buffer is: four periods of the largest size.
const PERIOD_BYTES: u32 = CAPTURE_MAX_PERIOD_SIZE;
const BUFFER_BYTES: u32 = CAPTURE_MAX_NUM_PERIODS * CAPTURE_MAX_PERIOD_SIZE;
const PAGE: u32 = 4096;
/// How often an idle driver asks whether someone listens.
const IDLE_POLL_MS: i32 = 200;
/// `npk_wait` bit: the bound device's interrupt fired.
const WAIT_IRQ: i32 = 2;

struct Acp {
    mmio: i32,
}

impl Acp {
    fn rd(&self, reg: u32) -> u32 {
        npk_sys::mmio_read32(self.mmio, reg as i32) as u32
    }
    fn wr(&self, val: u32, reg: u32) {
        npk_sys::mmio_write32(self.mmio, reg as i32, val as i32);
    }
}

fn udelay(us: u32) {
    let end = npk_sys::now_us() + us as i64;
    while npk_sys::now_us() < end {
        core::hint::spin_loop();
    }
}

/// `rn_acp_power_on`.
fn rn_acp_power_on(a: &Acp) -> Result<(), ()> {
    let val = a.rd(ACP_PGFSM_STATUS);
    if val == 0 {
        return Ok(());
    }
    if val & ACP_PGFSM_STATUS_MASK != ACP_POWER_ON_IN_PROGRESS {
        a.wr(ACP_PGFSM_CNTL_POWER_ON_MASK, ACP_PGFSM_CONTROL);
    }
    for _ in 1..500 {
        if a.rd(ACP_PGFSM_STATUS) == 0 {
            return Ok(());
        }
        udelay(1);
    }
    Err(())
}

/// `rn_acp_reset`.
fn rn_acp_reset(a: &Acp) -> Result<(), ()> {
    a.wr(1, ACP_SOFT_RESET);
    for _ in 1..500 {
        if a.rd(ACP_SOFT_RESET) & ACP_SOFT_RESET_SOFTRESET_AUDDONE_MASK != 0 {
            break;
        }
        core::hint::spin_loop();
    }
    a.wr(0, ACP_SOFT_RESET);
    for _ in 1..500 {
        if a.rd(ACP_SOFT_RESET) == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(())
}

/// `rn_acp_enable_interrupts`.
fn rn_acp_enable_interrupts(a: &Acp) {
    a.wr(0x01, ACP_EXTERNAL_INTR_ENB);
    let ext_intr_ctrl = a.rd(ACP_EXTERNAL_INTR_CNTL) | ACP_ERROR_MASK;
    a.wr(ext_intr_ctrl, ACP_EXTERNAL_INTR_CNTL);
}

/// `rn_acp_disable_interrupts`.
fn rn_acp_disable_interrupts(a: &Acp) {
    a.wr(ACP_EXT_INTR_STAT_CLEAR_MASK, ACP_EXTERNAL_INTR_STAT);
    a.wr(0x00, ACP_EXTERNAL_INTR_ENB);
}

/// `rn_acp_init`.
fn rn_acp_init(a: &Acp) -> Result<(), &'static str> {
    rn_acp_power_on(a).map_err(|_| "ACP power on failed")?;
    a.wr(0x01, ACP_CONTROL);
    rn_acp_reset(a).map_err(|_| "ACP reset failed")?;
    a.wr(0x03, ACP_CLKMUX_SEL);
    rn_acp_enable_interrupts(a);
    Ok(())
}

/// `rn_acp_deinit`, with `acp_power_gating` 0: the block stays powered.
fn rn_acp_deinit(a: &Acp) {
    rn_acp_disable_interrupts(a);
    if rn_acp_reset(a).is_err() {
        log("[audio_acp] ACP reset failed\n");
    }
    a.wr(0x00, ACP_CLKMUX_SEL);
    a.wr(0x00, ACP_CONTROL);
}

/// `init_pdm_ring_buffer`.
fn init_pdm_ring_buffer(a: &Acp, physical_addr: u32, buffer_size: u32, watermark_size: u32) {
    a.wr(physical_addr, ACP_WOV_RX_RINGBUFADDR);
    a.wr(buffer_size, ACP_WOV_RX_RINGBUFSIZE);
    a.wr(watermark_size, ACP_WOV_RX_INTR_WATERMARK_SIZE);
    a.wr(0x01, ACPAXI2AXI_ATU_CTRL);
}

/// `enable_pdm_clock`.
fn enable_pdm_clock(a: &Acp) {
    a.wr(ACP_PDM_CLK_FREQ_MASK, ACP_WOV_CLK_CTRL);
    let mut pdm_ctrl = a.rd(ACP_WOV_MISC_CTRL);
    pdm_ctrl &= !ACP_WOV_GAIN_CONTROL;
    pdm_ctrl |= (PDM_GAIN.min(3) << 3) & ACP_WOV_GAIN_CONTROL;
    a.wr(pdm_ctrl, ACP_WOV_MISC_CTRL);
}

/// `enable_pdm_interrupts`.
fn enable_pdm_interrupts(a: &Acp) {
    let v = a.rd(ACP_EXTERNAL_INTR_CNTL) | PDM_DMA_INTR_MASK;
    a.wr(v, ACP_EXTERNAL_INTR_CNTL);
}

/// `disable_pdm_interrupts`.
fn disable_pdm_interrupts(a: &Acp) {
    let v = a.rd(ACP_EXTERNAL_INTR_CNTL) & !PDM_DMA_INTR_MASK;
    a.wr(v, ACP_EXTERNAL_INTR_CNTL);
}

/// `check_pdm_dma_status`.
fn check_pdm_dma_status(a: &Acp) -> bool {
    a.rd(ACP_WOV_PDM_ENABLE) & ACP_PDM_ENABLE != 0 && a.rd(ACP_WOV_PDM_DMA_ENABLE) & ACP_PDM_DMA_EN_STATUS != 0
}

/// `start_pdm_dma`.
fn start_pdm_dma(a: &Acp) -> Result<(), ()> {
    enable_pdm_clock(a);
    a.wr(0x01, ACP_WOV_PDM_ENABLE);
    a.wr(0x01, ACP_WOV_PDM_DMA_ENABLE);
    for _ in 1..ACP_COUNTER {
        if a.rd(ACP_WOV_PDM_DMA_ENABLE) & 0x02 == ACP_PDM_DMA_EN_STATUS {
            return Ok(());
        }
        udelay(DELAY_US);
    }
    Err(())
}

/// `stop_pdm_dma`.
fn stop_pdm_dma(a: &Acp) -> Result<(), ()> {
    let pdm_enable = a.rd(ACP_WOV_PDM_ENABLE);
    let pdm_dma_enable = a.rd(ACP_WOV_PDM_DMA_ENABLE);
    if pdm_dma_enable & 0x01 != 0 {
        a.wr(0x02, ACP_WOV_PDM_DMA_ENABLE);
        let mut stopped = false;
        for _ in 1..ACP_COUNTER {
            if a.rd(ACP_WOV_PDM_DMA_ENABLE) & 0x02 == 0x00 {
                stopped = true;
                break;
            }
            udelay(DELAY_US);
        }
        if !stopped {
            return Err(());
        }
    }
    if pdm_enable == ACP_PDM_ENABLE {
        a.wr(ACP_PDM_DISABLE, ACP_WOV_PDM_ENABLE);
    }
    a.wr(0x01, ACP_WOV_PDM_FIFO_FLUSH);
    Ok(())
}

/// `config_acp_dma`: the ring's pages into the ACP's address translation.
fn config_acp_dma(a: &Acp, dma_addr: u64, num_pages: u32) {
    a.wr(ACP_SRAM_PTE_OFFSET | (1 << 31), ACPAXI2AXI_ATU_BASE_ADDR_GRP_1);
    a.wr(PAGE_SIZE_4K_ENABLE, ACPAXI2AXI_ATU_PAGE_SIZE_GRP_1);
    let mut addr = dma_addr;
    for page_idx in 0..num_pages {
        let val = page_idx * 8;
        a.wr(addr as u32, ACP_SCRATCH_REG_0 + val);
        a.wr((addr >> 32) as u32 | (1 << 31), ACP_SCRATCH_REG_0 + val + 4);
        addr += PAGE as u64;
    }
}

/// `acp_pdm_get_byte_count`.
fn acp_pdm_get_byte_count(a: &Acp) -> u64 {
    let high = a.rd(ACP_WOV_RX_LINEARPOSITIONCNTR_HIGH) as u64;
    let low = a.rd(ACP_WOV_RX_LINEARPOSITIONCNTR_LOW) as u64;
    high << 32 | low
}

/// `acp_pdm_dma_open` + `hw_params` + `trigger(START)`: the ring set up
/// and the DMA running. The byte count at the start.
fn capture_start(a: &Acp, ring_phys: u64) -> Option<u64> {
    enable_pdm_interrupts(a);
    config_acp_dma(a, ring_phys, BUFFER_BYTES.div_ceil(PAGE));
    init_pdm_ring_buffer(a, MEM_WINDOW_START, BUFFER_BYTES, PERIOD_BYTES);
    // `TWO_CH`: channel mask 0.
    a.wr(0x00, ACP_WOV_PDM_NO_OF_CHANNELS);
    a.wr(PDM_DECIMATION_FACTOR, ACP_WOV_PDM_DECIMATION_FACTOR);
    let start = acp_pdm_get_byte_count(a);
    if !check_pdm_dma_status(a) && start_pdm_dma(a).is_err() {
        log("[audio_acp] PDM DMA did not start\n");
        disable_pdm_interrupts(a);
        return None;
    }
    Some(start)
}

/// `trigger(STOP)` + `acp_pdm_dma_close`.
fn capture_stop(a: &Acp) {
    if check_pdm_dma_status(a) && stop_pdm_dma(a).is_err() {
        log("[audio_acp] PDM DMA did not stop\n");
    }
    disable_pdm_interrupts(a);
}

/// `pdm_irq_handler`: acknowledges the PDM interrupt; true if it was one.
fn pdm_irq(a: &Acp) -> bool {
    if a.rd(ACP_EXTERNAL_INTR_STAT) & (1 << PDM_DMA_STAT) != 0 {
        a.wr(1 << PDM_DMA_STAT, ACP_EXTERNAL_INTR_STAT);
        return true;
    }
    false
}

/// The bound function's bus/device/function and that of the bridge above
/// it, by config space (what `ACPI_HANDLE` finds through the companion).
fn pci_place() -> Option<(u32, u32)> {
    let ids = npk_sys::pci_read_config(0) as u32;
    let class = npk_sys::pci_read_config(0x08) as u32;
    let mut me = None;
    'scan: for bus in 0..=255u32 {
        for dev in 0..32u32 {
            if npk_sys::pci_read_config_at((bus << 8 | dev << 3) as i32, 0) as u32 == 0xFFFF_FFFF {
                continue;
            }
            for func in 0..8u32 {
                let bdf = (bus << 8 | dev << 3 | func) as i32;
                if npk_sys::pci_read_config_at(bdf, 0) as u32 == ids
                    && npk_sys::pci_read_config_at(bdf, 0x08) as u32 == class {
                    me = Some(bdf as u32);
                    break 'scan;
                }
            }
        }
    }
    let me = me?;
    let bus = me >> 8;
    for dev in 0..32u32 {
        for func in 0..8u32 {
            let bdf = (dev << 3 | func) as i32;
            let id = npk_sys::pci_read_config_at(bdf, 0) as u32;
            if id == 0xFFFF_FFFF || id == 0 {
                continue;
            }
            let header = (npk_sys::pci_read_config_at(bdf, 0x0C) as u32 >> 16) & 0x7F;
            let secondary = (npk_sys::pci_read_config_at(bdf, 0x18) as u32 >> 8) & 0xFF;
            if header == 1 && secondary == bus {
                return Some((me, bdf as u32));
            }
        }
    }
    None
}

/// `_ADR` of a PCI function: device in the high word, function in the low.
fn adr(bdf: u32) -> u64 {
    ((bdf >> 3 & 0x1F) as u64) << 16 | (bdf & 7) as u64
}

struct FirmwareAccess;
impl Ec for FirmwareAccess {
    fn read(&mut self, _a: u8) -> u8 { 0 }
    fn write(&mut self, _a: u8, _v: u8) {}
    fn mem_read(&mut self, addr: u64) -> Option<u8> {
        let v = npk_sys::acpi_mem_read((addr >> 32) as i32, addr as u32 as i32);
        if v < 0 { None } else { Some(v as u8) }
    }
    fn note(&mut self, _s: &str) {}
}

/// `acpi_evaluate_integer(handle, "_WOV")` on this function's ACPI node:
/// the device under the bridge's node whose `_ADR` is ours. None when the
/// firmware has no such node or method.
fn dmic_status() -> Option<u64> {
    let (me, bridge) = pci_place()?;
    let dsdt = npk_sys::read_all(npk_sys::acpi_dsdt).filter(|d| !d.is_empty())?;
    let mut ns = Namespace::load(&dsdt).ok()?;
    for i in 0..32 {
        let Some(t) = npk_sys::read_all(|b| npk_sys::acpi_table(*b"SSDT", i, b)).filter(|t| !t.is_empty()) else {
            break;
        };
        let _ = ns.load_more(&t);
    }
    let mut ec = FirmwareAccess;
    ns.resolve_conditionals(&mut ec);
    let candidates = ns.find_by_segment("_WOV");
    let mut m = Machine::new(&ns, &mut ec);
    for wov in candidates {
        let mut dev = wov.clone();
        dev.pop();
        let mut parent = dev.clone();
        parent.pop();
        let is = |m: &mut Machine, p: &aml_core::Path, want: u64| matches!(m.eval_child(p, "_ADR"), Ok(Value::Int(v)) if v == want);
        if is(&mut m, &dev, adr(me)) && is(&mut m, &parent, adr(bridge)) {
            return match m.eval_child(&dev, "_WOV") {
                Ok(Value::Int(v)) => Some(v),
                _ => None,
            };
        }
    }
    None
}

/// The first ACP 3.x of Renoir (`snd_rn_acp_ids`, `pci->revision == 1`).
fn bind() -> bool {
    for i in 0..8 {
        if npk_sys::pci_bind_class_n(0x04, 0x80, i) != 0 {
            return false;
        }
        let ids = npk_sys::pci_read_config(0) as u32;
        let rev = npk_sys::pci_read_config(0x08) as u32 & 0xFF;
        if ids == (ACP_DEVICE_ID << 16 | 0x1022) && rev == 0x01 {
            return true;
        }
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    log(concat!("[audio_acp] ", env!("CARGO_PKG_VERSION"), " - Renoir ACP digital microphone\n"));
    if !bind() {
        log("[audio_acp] no Renoir ACP - exit\n");
        return;
    }
    npk_sys::pci_enable_bus_master();
    let bar = npk_sys::pci_bar_size(0);
    let need = (ACP_REG_END - ACP_PHY_BASE_ADDRESS) as i64;
    if bar < need {
        log("[audio_acp] BAR 0 too small - exit\n");
        return;
    }
    let mmio = npk_sys::mmio_map_bar(0, (need as u32).div_ceil(PAGE) as i32);
    if mmio < 0 {
        log("[audio_acp] BAR 0 not mapped - exit\n");
        return;
    }
    let a = Acp { mmio };
    if let Err(e) = rn_acp_init(&a) {
        log("[audio_acp] ");
        log(e);
        log("\n");
        return;
    }
    match dmic_status() {
        Some(v) if v != 0 => {}
        Some(_) => {
            log("[audio_acp] _WOV says no digital microphone - exit\n");
            rn_acp_deinit(&a);
            return;
        }
        None => {
            log("[audio_acp] no _WOV for this ACP - exit\n");
            rn_acp_deinit(&a);
            return;
        }
    }
    let ring = npk_sys::dma_alloc(BUFFER_BYTES.div_ceil(PAGE) as i32);
    if ring < 0 {
        log("[audio_acp] DMA alloc failed - exit\n");
        rn_acp_deinit(&a);
        return;
    }
    let ring_phys = npk_sys::dma_phys_addr(ring) as u64;
    let irq = npk_sys::irq_register(0) >= 0;
    if npk_sys::audio_capture_device_format(RATE, ENCODING_S32, CHANNELS) != 0 {
        log("[audio_acp] capture format refused - exit\n");
        rn_acp_deinit(&a);
        return;
    }
    let handle = npk_sys::device_register(npk_sys::DEVICE_AUDIO_IN, npk_sys::RANK_BUILTIN,
                                          "acp-1022:15e2-pdm", "Built-in microphone");
    if handle < 0 {
        log("[audio_acp] not in the device register - exit\n");
        rn_acp_deinit(&a);
        return;
    }
    log(if irq { "[audio_acp] digital microphone ready (MSI)\n" } else { "[audio_acp] digital microphone ready (polled)\n" });

    let mut buf = alloc::vec![0u8; BUFFER_BYTES as usize];
    // The linear counter at the start and the bytes handed on since: the
    // ring position is their difference modulo the ring
    // (`acp_pdm_dma_pointer`).
    let mut running: Option<(u64, u64)> = None;
    loop {
        let wanted = npk_sys::audio_capture_wanted() > 0;
        match (running, wanted) {
            (None, true) => {
                running = capture_start(&a, ring_phys).map(|start| (start, 0));
                if running.is_some() {
                    log("[audio_acp] recording\n");
                }
            }
            (Some(_), false) => {
                capture_stop(&a);
                running = None;
                log("[audio_acp] stopped\n");
            }
            _ => {}
        }
        let Some((start, done)) = running.as_mut() else {
            npk_sys::sleep(IDLE_POLL_MS);
            continue;
        };
        if irq {
            npk_sys::wait(WAIT_IRQ, (PERIOD_BYTES * 1000 / (RATE as u32 * 8)) as i32 + 10);
        } else {
            npk_sys::sleep(10);
        }
        pdm_irq(&a);
        let now = acp_pdm_get_byte_count(&a).saturating_sub(*start);
        let mut new = now.saturating_sub(*done);
        if new > BUFFER_BYTES as u64 {
            // Fell behind by more than the ring: skip to what is still there.
            *done = now - BUFFER_BYTES as u64;
            new = BUFFER_BYTES as u64;
        }
        let new = new as u32 - new as u32 % 8;
        if new == 0 {
            continue;
        }
        let at = (*done % BUFFER_BYTES as u64) as u32;
        let first = new.min(BUFFER_BYTES - at);
        npk_sys::dma_read(ring, at as i32, &mut buf[..first as usize]);
        if first < new {
            npk_sys::dma_read(ring, 0, &mut buf[first as usize..new as usize]);
        }
        npk_sys::audio_capture_push(&buf[..new as usize]);
        *done += new as u64;
    }
}
