//! RP1's two USB host controllers, Synopsys DWC_usb3 3.30b in host mode,
//! after Linux `drivers/usb/dwc3/core.c` and `host.c`: `dwc3_core_probe`
//! up to `dwc3_host_init`, which hands the xHCI registers to the xHCI
//! driver with the quirks `xhci-sg-trb-cache-size-quirk` and
//! `write-64-hi-lo-quirk`, and `dwc3_xhci_plat_start`.
//!
//! The branches this core and its device tree node reach: UTMI HS PHY,
//! no ULPI, no generic or legacy PHY drivers, no frame length or reference
//! clock adjustment, no `snps,incr-burst-type-adjustment`, no event
//! buffers (host only).

use crate::hw::{Block, Hw};

const DWC3_GSBUSCFG0: u64 = 0xc100;
const DWC3_GSBUSCFG1: u64 = 0xc104;
const DWC3_GTXTHRCFG: u64 = 0xc108;
const DWC3_GRXTHRCFG: u64 = 0xc10c;
const DWC3_GCTL: u64 = 0xc110;
const DWC3_GUCTL1: u64 = 0xc11c;
const DWC3_GSNPSID: u64 = 0xc120;
const DWC3_GUID: u64 = 0xc128;
const DWC3_GUCTL2: u64 = 0xc19c;
const DWC3_GHWPARAMS0: u64 = 0xc140;
const DWC3_GHWPARAMS8: u64 = 0xc600;
fn dwc3_gusb2phycfg(n: u32) -> u64 { 0xc200 + 4 * n as u64 }
fn dwc3_gusb3pipectl(n: u32) -> u64 { 0xc2c0 + 4 * n as u64 }
const DWC3_DCTL: u64 = 0xc704;

const DWC3_GSBUSCFG1_PIPETRANSLIMIT_MASK: u32 = 0xf << 8;
const DWC3_GTXTHRCFG_PKTCNTSEL: u32 = 1 << 29;
const DWC3_GRXTHRCFG_PKTCNTSEL: u32 = 1 << 29;
const DWC3_GCTL_SCALEDOWN_MASK: u32 = 3 << 4;
const DWC3_GCTL_DISSCRAMBLE: u32 = 1 << 3;
const DWC3_GCTL_DSBLCLKGTNG: u32 = 1 << 0;
const DWC3_GCTL_SOFITPSYNC: u32 = 1 << 10;
const DWC3_GCTL_U2RSTECN: u32 = 1 << 16;
const DWC3_GCTL_PRTCAPDIR_MASK: u32 = 3 << 12;
const DWC3_GCTL_PRTCAP_HOST: u32 = 1;
const DWC3_GUCTL1_DEV_DECOUPLE_L1L2_EVT: u32 = 1 << 31;
const DWC3_GUCTL1_TX_IPGAP_LINECHECK_DIS: u32 = 1 << 28;
const DWC3_GUCTL1_DEV_FORCE_20_CLK_FOR_30_CLK: u32 = 1 << 26;
const DWC3_GUCTL1_DEV_L1_EXIT_BY_HW: u32 = 1 << 24;
const DWC3_GUCTL1_NAK_PER_ENH_FS: u32 = 1 << 19;
const DWC3_GUCTL1_NAK_PER_ENH_HS: u32 = 1 << 18;
const DWC3_GUCTL1_PARKMODE_DISABLE_SS: u32 = 1 << 17;
const DWC3_GUCTL1_PARKMODE_DISABLE_HS: u32 = 1 << 16;
const DWC3_GUCTL1_PARKMODE_DISABLE_FSLS: u32 = 1 << 15;
const DWC3_GUCTL2_RST_ACTBITLATER: u32 = 1 << 14;
const DWC3_GUSB2PHYCFG_U2_FREECLK_EXISTS: u32 = 1 << 30;
const DWC3_GUSB2PHYCFG_ENBLSLPM: u32 = 1 << 8;
const DWC3_GUSB2PHYCFG_SUSPHY: u32 = 1 << 6;
const DWC3_GUSB3PIPECTL_DISRXDETINP3: u32 = 1 << 28;
const DWC3_GUSB3PIPECTL_UX_EXIT_PX: u32 = 1 << 27;
const DWC3_GUSB3PIPECTL_SUSPHY: u32 = 1 << 17;
const DWC3_DCTL_RUN_STOP: u32 = 1 << 31;
const DWC3_DCTL_CSFTRST: u32 = 1 << 30;
const DWC3_DCTL_ULSTCHNGREQ_MASK: u32 = 0x0f << 5;
const DWC3_GHWPARAMS6_EN_FPGA: u32 = 1 << 7;

const DWC3_GHWPARAMS0_MODE_HOST: u32 = 1;
const DWC3_GHWPARAMS0_MODE_DRD: u32 = 2;
const DWC3_GHWPARAMS1_EN_PWROPT_CLK: u32 = 1;
const DWC3_GHWPARAMS1_EN_PWROPT_HIB: u32 = 2;
const DWC3_GCTL_GBLHIBERNATIONEN: u32 = 1 << 1;

/// `DWC3_GSNPSID_MASK` 0xffff0000 values for DWC_usb3.
const DWC3_GSNPSID_DWC3: u32 = 0x5533;
/// `DWC3_REVISION_*`: the low half of GSNPSID.
const REV_194A: u32 = 0x194a;
const REV_250A: u32 = 0x250a;
const REV_290A: u32 = 0x290a;
const REV_300A: u32 = 0x300a;
const REV_310A: u32 = 0x310a;

const XHCI_HCSPARAMS1: u64 = 0x04;
const XHCI_HCCPARAMS1: u64 = 0x10;
const XHCI_PORTSC_BASE: u64 = 0x400;
const PORT_POWER: u32 = 1 << 9;
const XHCI_EXT_CAPS_PROTOCOL: u32 = 2;

/// The quirks of a `snps,dwc3` node (`dwc3_get_properties`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Config {
    /// `snps,dis_rxdet_inp3_quirk`.
    pub dis_rxdet_inp3: bool,
    /// `snps,enhanced-nak-fs-quirk`.
    pub enh_nak_fs: bool,
    /// `snps,enhanced-nak-hs-quirk`.
    pub enh_nak_hs: bool,
    /// `snps,parkmode-disable-ss-quirk`.
    pub parkmode_disable_ss: bool,
    /// `snps,parkmode-disable-hs-quirk`.
    pub parkmode_disable_hs: bool,
    /// `snps,parkmode-disable-fsls-quirk`.
    pub parkmode_disable_fsls: bool,
    /// `snps,dis-tx-ipgap-linecheck-quirk`.
    pub dis_tx_ipgap_linecheck: bool,
    /// `snps,dis_enblslpm_quirk`.
    pub dis_enblslpm: bool,
    /// `snps,dis-u2-freeclk-exists-quirk`.
    pub dis_u2_freeclk_exists: bool,
    /// `snps,dis_u2_susphy_quirk`.
    pub dis_u2_susphy: bool,
    /// `snps,dis_u3_susphy_quirk`.
    pub dis_u3_susphy: bool,
    /// `snps,axi-pipe-limit`, 0 without one.
    pub axi_pipe_limit: u8,
    /// `snps,rx-thr-num-pkt`, `snps,rx-max-burst`.
    pub rx_thr_num_pkt: u8,
    pub rx_max_burst: u8,
    /// `snps,tx-thr-num-pkt`, `snps,tx-max-burst`.
    pub tx_thr_num_pkt: u8,
    pub tx_max_burst: u8,
    /// `snps,tx-thr-num-pkt-prd`, `snps,tx-max-burst-prd`.
    pub tx_thr_num_pkt_prd: u8,
    pub tx_max_burst_prd: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// GSNPSID is not a DWC_usb3 core.
    NotDwc3(u32),
    /// The core did not leave soft reset.
    SoftResetTimeout,
}

/// A controller at offset `base` of RP1's BAR1, in host mode.
#[derive(Clone, Copy, Debug)]
pub struct Dwc3 {
    pub base: u64,
    /// GSNPSID's revision half, e.g. 0x330b.
    pub revision: u32,
    pub hwparams: [u32; 9],
    pub num_usb2_ports: u32,
    pub num_usb3_ports: u32,
    pub is_fpga: bool,
    pub cfg: Config,
}

impl Dwc3 {
    fn r(&self, hw: &mut impl Hw, off: u64) -> u32 { hw.r32(Block::Rp1Bar1, self.base + off) }
    fn w(&self, hw: &mut impl Hw, off: u64, v: u32) { hw.w32(Block::Rp1Bar1, self.base + off, v) }

    fn rev_at_least(&self, rev: u32) -> bool { self.revision >= rev }

    /// `dwc3_get_num_ports`: count the ports of each USB revision in the
    /// xHCI Supported Protocol capabilities.
    fn get_num_ports(&mut self, hw: &mut impl Hw) {
        let hcc = self.r(hw, XHCI_HCCPARAMS1);
        let mut offset = ((hcc >> 16) as u64) << 2;
        if offset == 0 { return; }
        let (mut u2, mut u3) = (0, 0);
        // xhci_find_next_ext_cap: start at the first capability, then
        // follow the next pointers, testing each for the protocol ID.
        loop {
            let val = self.r(hw, offset);
            if val & 0xff == XHCI_EXT_CAPS_PROTOCOL {
                let major = val >> 24;
                let ports = self.r(hw, offset + 8);
                let count = (ports >> 8) & 0xff;
                match major {
                    0x03 => u3 += count,
                    0x02 => u2 += count,
                    _ => {}
                }
            }
            let next = (val >> 8) & 0xff;
            if next == 0 { break; }
            offset += (next as u64) << 2;
        }
        self.num_usb2_ports = u2;
        self.num_usb3_ports = u3;
    }

    /// `dwc3_ss_phy_setup`.
    fn ss_phy_setup(&self, hw: &mut impl Hw, index: u32) {
        let mut reg = self.r(hw, dwc3_gusb3pipectl(index));
        reg &= !DWC3_GUSB3PIPECTL_UX_EXIT_PX;
        reg &= !DWC3_GUSB3PIPECTL_SUSPHY;
        if self.cfg.dis_rxdet_inp3 {
            reg |= DWC3_GUSB3PIPECTL_DISRXDETINP3;
        }
        self.w(hw, dwc3_gusb3pipectl(index), reg);
    }

    /// `dwc3_hs_phy_setup` for a UTMI PHY without `phy_type`.
    fn hs_phy_setup(&self, hw: &mut impl Hw, index: u32) {
        let mut reg = self.r(hw, dwc3_gusb2phycfg(index));
        reg &= !DWC3_GUSB2PHYCFG_SUSPHY;
        if self.cfg.dis_enblslpm {
            reg &= !DWC3_GUSB2PHYCFG_ENBLSLPM;
        } else {
            reg |= DWC3_GUSB2PHYCFG_ENBLSLPM;
        }
        if self.cfg.dis_u2_freeclk_exists {
            reg &= !DWC3_GUSB2PHYCFG_U2_FREECLK_EXISTS;
        }
        self.w(hw, dwc3_gusb2phycfg(index), reg);
    }

    /// `dwc3_phy_setup`.
    fn phy_setup(&self, hw: &mut impl Hw) {
        for i in 0..self.num_usb3_ports {
            self.ss_phy_setup(hw, i);
        }
        for i in 0..self.num_usb2_ports {
            self.hs_phy_setup(hw, i);
        }
    }

    /// `dwc3_enable_susphy`: the PHYs may suspend (after core init, and
    /// again from `dwc3_xhci_plat_start`), or not.
    pub fn enable_susphy(&self, hw: &mut impl Hw, enable: bool) {
        for i in 0..self.num_usb3_ports {
            let mut reg = self.r(hw, dwc3_gusb3pipectl(i));
            if enable && !self.cfg.dis_u3_susphy {
                reg |= DWC3_GUSB3PIPECTL_SUSPHY;
            } else {
                reg &= !DWC3_GUSB3PIPECTL_SUSPHY;
            }
            self.w(hw, dwc3_gusb3pipectl(i), reg);
        }
        for i in 0..self.num_usb2_ports {
            let mut reg = self.r(hw, dwc3_gusb2phycfg(i));
            if enable && !self.cfg.dis_u2_susphy {
                reg |= DWC3_GUSB2PHYCFG_SUSPHY;
            } else {
                reg &= !DWC3_GUSB2PHYCFG_SUSPHY;
            }
            self.w(hw, dwc3_gusb2phycfg(i), reg);
        }
    }

    /// `dwc3_core_soft_reset` before the role is host: CSFTRST, then wait
    /// up to 1000 x 1 us for it to clear.
    fn core_soft_reset(&self, hw: &mut impl Hw) -> Result<(), Error> {
        let mut reg = self.r(hw, DWC3_DCTL);
        reg |= DWC3_DCTL_CSFTRST;
        reg &= !DWC3_DCTL_RUN_STOP;
        // dwc3_gadget_dctl_write_safe.
        self.w(hw, DWC3_DCTL, reg & !DWC3_DCTL_ULSTCHNGREQ_MASK);
        for _ in 0..1000 {
            if self.r(hw, DWC3_DCTL) & DWC3_DCTL_CSFTRST == 0 {
                return Ok(());
            }
            hw.delay_us(1);
        }
        Err(Error::SoftResetTimeout)
    }

    /// `dwc3_core_setup_global_control` for a host-only core.
    fn setup_global_control(&mut self, hw: &mut impl Hw) {
        let mut reg = self.r(hw, DWC3_GCTL);
        reg &= !DWC3_GCTL_SCALEDOWN_MASK;
        let power_opt = (self.hwparams[1] >> 24) & 3;
        match power_opt {
            DWC3_GHWPARAMS1_EN_PWROPT_CLK => {
                // Clock gating stays on outside 2.10a..2.50a hosts.
                if (0x210a..=0x250a).contains(&self.revision) {
                    reg |= DWC3_GCTL_DSBLCLKGTNG | DWC3_GCTL_SOFITPSYNC;
                } else {
                    reg &= !DWC3_GCTL_DSBLCLKGTNG;
                }
            }
            DWC3_GHWPARAMS1_EN_PWROPT_HIB => reg |= DWC3_GCTL_GBLHIBERNATIONEN,
            _ => {}
        }
        self.is_fpga = self.hwparams[6] & DWC3_GHWPARAMS6_EN_FPGA != 0;
        // No disable_scramble_quirk.
        reg &= !DWC3_GCTL_DISSCRAMBLE;
        if !self.rev_at_least(0x190a) {
            reg |= DWC3_GCTL_U2RSTECN;
        }
        self.w(hw, DWC3_GCTL, reg);
    }

    /// `dwc3_set_incr_burst_type` without `snps,incr-burst-type-adjustment`:
    /// the register is read and left alone.
    fn set_incr_burst_type(&self, hw: &mut impl Hw) {
        self.r(hw, DWC3_GSBUSCFG0);
    }

    /// `dwc3_set_axi_pipe_limit`.
    fn set_axi_pipe_limit(&self, hw: &mut impl Hw) {
        let lim = self.cfg.axi_pipe_limit as u32;
        if lim == 0 || lim > 16 { return; }
        let mut cfg = self.r(hw, DWC3_GSBUSCFG1);
        cfg &= !DWC3_GSBUSCFG1_PIPETRANSLIMIT_MASK;
        cfg |= ((lim - 1) & 0xf) << 8;
        self.w(hw, DWC3_GSBUSCFG1, cfg);
    }

    /// `dwc3_config_threshold` for DWC_usb3.
    fn config_threshold(&self, hw: &mut impl Hw) {
        let (rx_thr, rx_burst) = (self.cfg.rx_thr_num_pkt as u32, self.cfg.rx_max_burst as u32);
        if rx_thr != 0 && rx_burst != 0 {
            let mut reg = self.r(hw, DWC3_GRXTHRCFG);
            reg |= DWC3_GRXTHRCFG_PKTCNTSEL;
            reg &= !(0xf << 24);
            reg |= (rx_thr & 0xf) << 24;
            reg &= !(0x1f << 19);
            reg |= (rx_burst & 0x1f) << 19;
            self.w(hw, DWC3_GRXTHRCFG, reg);
        }
        self.tx_threshold(hw, self.cfg.tx_thr_num_pkt, self.cfg.tx_max_burst);
    }

    fn tx_threshold(&self, hw: &mut impl Hw, thr: u8, burst: u8) {
        if thr == 0 || burst == 0 { return; }
        let mut reg = self.r(hw, DWC3_GTXTHRCFG);
        reg |= DWC3_GTXTHRCFG_PKTCNTSEL;
        reg &= !(0xf << 24);
        reg |= (thr as u32 & 0xf) << 24;
        reg &= !(0xff << 16);
        reg |= (burst as u32 & 0xff) << 16;
        self.w(hw, DWC3_GTXTHRCFG, reg);
    }

    /// `dwc3_core_init`. `guid` goes to GUID, where Linux writes its
    /// version for software on the other side to read.
    fn core_init(&mut self, hw: &mut impl Hw, guid: u32) -> Result<(), Error> {
        self.phy_setup(hw);
        // dwc3_phy_init: the PHYs may suspend once initialised.
        if self.rev_at_least(REV_194A + 1) {
            self.enable_susphy(hw, true);
        }
        self.core_soft_reset(hw)?;
        self.w(hw, DWC3_GUID, guid);
        self.setup_global_control(hw);
        self.set_incr_burst_type(hw);
        self.set_axi_pipe_limit(hw);

        if self.rev_at_least(REV_310A) {
            let reg = self.r(hw, DWC3_GUCTL2) | DWC3_GUCTL2_RST_ACTBITLATER;
            self.w(hw, DWC3_GUCTL2, reg);
        }
        if self.rev_at_least(REV_250A) {
            let mut reg = self.r(hw, DWC3_GUCTL1);
            if self.rev_at_least(REV_290A) { reg |= DWC3_GUCTL1_DEV_L1_EXIT_BY_HW; }
            if self.rev_at_least(REV_300A) { reg |= DWC3_GUCTL1_DEV_DECOUPLE_L1L2_EVT; }
            let c = &self.cfg;
            if c.dis_tx_ipgap_linecheck { reg |= DWC3_GUCTL1_TX_IPGAP_LINECHECK_DIS; }
            if c.enh_nak_fs { reg |= DWC3_GUCTL1_NAK_PER_ENH_FS; }
            if c.enh_nak_hs { reg |= DWC3_GUCTL1_NAK_PER_ENH_HS; }
            if c.parkmode_disable_ss { reg |= DWC3_GUCTL1_PARKMODE_DISABLE_SS; }
            if c.parkmode_disable_hs { reg |= DWC3_GUCTL1_PARKMODE_DISABLE_HS; }
            if c.parkmode_disable_fsls { reg |= DWC3_GUCTL1_PARKMODE_DISABLE_FSLS; }
            // maximum_speed is still unknown here: no forced 2.0 clock.
            if self.rev_at_least(REV_290A) { reg &= !DWC3_GUCTL1_DEV_FORCE_20_CLK_FOR_30_CLK; }
            self.w(hw, DWC3_GUCTL1, reg);
        }
        self.config_threshold(hw);
        self.tx_threshold(hw, self.cfg.tx_thr_num_pkt_prd, self.cfg.tx_max_burst_prd);
        Ok(())
    }

    /// `dwc3_set_prtcap` to host.
    fn set_prtcap_host(&self, hw: &mut impl Hw) {
        let mut reg = self.r(hw, DWC3_GCTL);
        let hw_mode = self.hwparams[0] & 7;
        if hw_mode == DWC3_GHWPARAMS0_MODE_DRD && (reg >> 12) & 3 != DWC3_GCTL_PRTCAP_HOST {
            self.enable_susphy(hw, false);
        }
        reg &= !DWC3_GCTL_PRTCAPDIR_MASK;
        reg |= DWC3_GCTL_PRTCAP_HOST << 12;
        self.w(hw, DWC3_GCTL, reg);
    }

    /// `dwc3_power_off_all_roothub_ports`: no VBUS glitch when the xHCI
    /// driver resets the controller.
    fn power_off_all_roothub_ports(&self, hw: &mut impl Hw) {
        let op_regs_base = (self.r(hw, 0) & 0xff) as u64;
        let port_num = self.r(hw, XHCI_HCSPARAMS1) >> 24;
        for i in 1..=port_num as u64 {
            let offset = op_regs_base + XHCI_PORTSC_BASE + 0x10 * (i - 1);
            let reg = self.r(hw, offset) & !PORT_POWER;
            self.w(hw, offset, reg);
        }
    }
}

/// `dwc3_probe` of a host-only core at `base` in BAR1, up to the point
/// where `dwc3_host_init` hands over to the xHCI driver.
pub fn probe(hw: &mut impl Hw, base: u64, cfg: Config, guid: u32) -> Result<Dwc3, Error> {
    let id = hw.r32(Block::Rp1Bar1, base + DWC3_GSNPSID);
    if id >> 16 != DWC3_GSNPSID_DWC3 {
        return Err(Error::NotDwc3(id));
    }
    let mut hwparams = [0u32; 9];
    for (i, p) in hwparams.iter_mut().enumerate().take(8) {
        *p = hw.r32(Block::Rp1Bar1, base + DWC3_GHWPARAMS0 + 4 * i as u64);
    }
    hwparams[8] = hw.r32(Block::Rp1Bar1, base + DWC3_GHWPARAMS8);
    let mut d = Dwc3 {
        base, revision: id & 0xffff, hwparams, num_usb2_ports: 0, num_usb3_ports: 0,
        is_fpga: false, cfg,
    };
    d.get_num_ports(hw);
    d.core_init(hw, guid)?;
    if d.hwparams[0] & 7 == DWC3_GHWPARAMS0_MODE_HOST || d.hwparams[0] & 7 == DWC3_GHWPARAMS0_MODE_DRD {
        d.set_prtcap_host(hw);
        d.power_off_all_roothub_ports(hw);
    }
    Ok(d)
}
