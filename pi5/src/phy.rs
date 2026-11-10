//! The Pi 5's Ethernet PHY, a Broadcom BCM54213PE on RGMII with internal
//! RX and TX delays (`rgmii-id`), after Linux `drivers/net/phy/broadcom.c`
//! and `bcm-phy-lib.c`, and the share of phylib (`phy_device.c`, `phy.c`,
//! `phy-c45.c`) and phylink this board reaches: polling (no PHY
//! interrupt), autonegotiation, EEE, no SFP, no WoL, no PHY timestamping.
//!
//! The framework is reduced to the decisions this PHY and MAC make; every
//! MDIO access it makes is kept, in its order. Clause 45 registers go over
//! Clause 22 (registers 13 and 14), as `mmd_phy_indirect` does.

/// One PHY's Clause 22 registers.
pub trait Mdio {
    fn read(&mut self, reg: u32) -> Result<u16, Error>;
    fn write(&mut self, reg: u32, v: u16) -> Result<(), Error>;
    fn delay_us(&mut self, us: u64);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The MDIO bus did not go idle (`-ETIMEDOUT`).
    Timeout,
    /// Something other than a BCM54213PE answers.
    Id(u32),
    /// Master/slave resolution failed (`-ENOLINK`).
    MasterSlave,
}

pub const PHY_ID_BCM54213PE: u32 = 0x600d_84a2;

const MII_BMCR: u32 = 0x00;
const MII_BMSR: u32 = 0x01;
const MII_PHYSID1: u32 = 0x02;
const MII_PHYSID2: u32 = 0x03;
const MII_ADVERTISE: u32 = 0x04;
const MII_LPA: u32 = 0x05;
const MII_CTRL1000: u32 = 0x09;
const MII_STAT1000: u32 = 0x0a;
const MII_MMD_CTRL: u32 = 0x0d;
const MII_MMD_DATA: u32 = 0x0e;
const MII_ESTATUS: u32 = 0x0f;
const MII_MMD_CTRL_NOINCR: u16 = 0x4000;

const BMCR_ANRESTART: u16 = 0x0200;
const BMCR_ISOLATE: u16 = 0x0400;
const BMCR_PDOWN: u16 = 0x0800;
const BMCR_ANENABLE: u16 = 0x1000;
const BMSR_LSTATUS: u16 = 0x0004;
const BMSR_ANEGCAPABLE: u16 = 0x0008;
const BMSR_ANEGCOMPLETE: u16 = 0x0020;
const BMSR_ESTATEN: u16 = 0x0100;
const BMSR_10HALF: u16 = 0x0800;
const BMSR_10FULL: u16 = 0x1000;
const BMSR_100HALF: u16 = 0x2000;
const BMSR_100FULL: u16 = 0x4000;
const ESTATUS_1000_THALF: u16 = 0x1000;
const ESTATUS_1000_TFULL: u16 = 0x2000;
const ESTATUS_1000_XFULL: u16 = 0x8000;
const ADVERTISE_10HALF: u16 = 0x0020;
const ADVERTISE_10FULL: u16 = 0x0040;
const ADVERTISE_100HALF: u16 = 0x0080;
const ADVERTISE_100FULL: u16 = 0x0100;
const ADVERTISE_100BASE4: u16 = 0x0200;
const ADVERTISE_PAUSE_CAP: u16 = 0x0400;
const ADVERTISE_PAUSE_ASYM: u16 = 0x0800;
const ADVERTISE_ALL: u16 = 0x01e0;
const LPA_LPACK: u16 = 0x4000;
const ADVERTISE_1000HALF: u16 = 0x0100;
const ADVERTISE_1000FULL: u16 = 0x0200;
const CTL1000_ENABLE_MASTER: u16 = 0x1000;
const LPA_1000HALF: u16 = 0x0400;
const LPA_1000FULL: u16 = 0x0800;
const LPA_1000MSFAIL: u16 = 0x8000;

const MDIO_MMD_PCS: u16 = 3;
const MDIO_MMD_AN: u16 = 7;
const MDIO_CTRL1: u16 = 0;
const MDIO_STAT1: u16 = 1;
const MDIO_PCS_EEE_ABLE: u16 = 20;
const MDIO_AN_EEE_ADV: u16 = 60;
const MDIO_AN_EEE_LPABLE: u16 = 61;
const MDIO_PCS_STAT1_CLKSTOP_CAP: u16 = 0x0040;
const MDIO_PCS_CTRL1_CLKSTOP_EN: u16 = 0x0400;
const MDIO_EEE_100TX: u16 = 0x0002;
const MDIO_EEE_1000T: u16 = 0x0004;
const MDIO_EEE_CAP1_MASK: u16 = 0x007e;

const MII_BCM54XX_ECR: u32 = 0x10;
const MII_BCM54XX_ECR_IM: u16 = 0x1000;
const MII_BCM54XX_EXP_DATA: u32 = 0x15;
const MII_BCM54XX_EXP_SEL: u32 = 0x17;
const MII_BCM54XX_AUX_CTL: u32 = 0x18;
const MII_BCM54XX_ISR: u32 = 0x1a;
const MII_BCM54XX_IMR: u32 = 0x1b;
const MII_BCM54XX_SHD: u32 = 0x1c;
const MII_BCM54XX_SHD_WRITE: u16 = 0x8000;
const MII_BCM54XX_INT_LINK: u16 = 0x0002;
const MII_BCM54XX_INT_SPEED: u16 = 0x0004;
const MII_BCM54XX_INT_DUPLEX: u16 = 0x0008;
const AUXCTL_SHDWSEL_AUXCTL: u16 = 0x00;
const AUXCTL_ACTL_TX_6DB: u16 = 0x0400;
const AUXCTL_ACTL_SMDSP_ENA: u16 = 0x0800;
const AUXCTL_SHDWSEL_MISC: u16 = 0x07;
const AUXCTL_SHDWSEL_MISC_RGMII_SKEW_EN: u16 = 0x0100;
const AUXCTL_MISC_WREN: u16 = 0x8000;
const AUXCTL_SHDWSEL_READ_SHIFT: u16 = 12;
const AUXCTL_SHDWSEL_MASK: u16 = 0x0007;
const BCM54XX_SHD_SCR3: u16 = 0x05;
const BCM54XX_SHD_SCR3_DLLAPD_DIS: u16 = 0x0002;
const BCM54XX_SHD_APD: u16 = 0x0a;
const BCM54XX_SHD_APD_EN: u16 = 0x0020;
const BCM54XX_SHD_LEDS1: u16 = 0x0d;
const BCM54XX_SHD_LEDS2: u16 = 0x0e;
const BCM54810_SHD_CLK_CTL: u16 = 0x3;
const BCM54810_SHD_CLK_CTL_GTXCLK_EN: u16 = 0x0200;
const BCM_LED_SRC_MULTICOLOR1: u16 = 0xa;
const BCM_EXP_MULTICOLOR: u16 = 0x0f04;
const BCM_LED_MULTICOLOR_IN_PHASE: u16 = 0x0100;
const BCM54XX_TOP_MISC_MII_BUF_CNTL0: u16 = 0x0d00;
const BCM54XX_MII_BUF_CNTL0_AUTOGREEEN_EN: u16 = 0x0001;
const BCM54XX_WOL_INT_STATUS: u16 = 0x0e94;
const MII_BCM54XX_EXP_EXP08: u16 = 0x0f08;
const EXP08_DAC_WAKE: u16 = 0x0200 | 0x0100;

/// Link modes, a subset of ethtool's.
pub mod modes {
    pub const TP: u32 = 1 << 0;
    pub const MII: u32 = 1 << 1;
    pub const AUTONEG: u32 = 1 << 2;
    pub const T10_HALF: u32 = 1 << 3;
    pub const T10_FULL: u32 = 1 << 4;
    pub const T100_HALF: u32 = 1 << 5;
    pub const T100_FULL: u32 = 1 << 6;
    pub const T1000_HALF: u32 = 1 << 7;
    pub const T1000_FULL: u32 = 1 << 8;
    pub const X1000_FULL: u32 = 1 << 9;
    pub const PAUSE: u32 = 1 << 10;
    pub const ASYM_PAUSE: u32 = 1 << 11;
    /// `PHY_EEE_CAP1_FEATURES` this subset holds.
    pub const EEE_CAP1: u32 = T100_FULL | T1000_FULL;
}
use modes::*;

/// `mii_eee_cap1_mod_linkmode_t` on the modes above.
fn eee_cap1_to_modes(v: u16) -> u32 {
    (if v & MDIO_EEE_100TX != 0 { T100_FULL } else { 0 }) | (if v & MDIO_EEE_1000T != 0 { T1000_FULL } else { 0 })
}

fn modes_to_eee_cap1(m: u32) -> u16 {
    (if m & T100_FULL != 0 { MDIO_EEE_100TX } else { 0 }) | (if m & T1000_FULL != 0 { MDIO_EEE_1000T } else { 0 })
}

/// `linkmode_adv_to_mii_adv_t`.
fn modes_to_adv(m: u32) -> u16 {
    let mut a = 0;
    if m & T10_HALF != 0 { a |= ADVERTISE_10HALF; }
    if m & T10_FULL != 0 { a |= ADVERTISE_10FULL; }
    if m & T100_HALF != 0 { a |= ADVERTISE_100HALF; }
    if m & T100_FULL != 0 { a |= ADVERTISE_100FULL; }
    if m & PAUSE != 0 { a |= ADVERTISE_PAUSE_CAP; }
    if m & ASYM_PAUSE != 0 { a |= ADVERTISE_PAUSE_ASYM; }
    a
}

/// `linkmode_adv_to_mii_ctrl1000_t`.
fn modes_to_ctrl1000(m: u32) -> u16 {
    (if m & T1000_HALF != 0 { ADVERTISE_1000HALF } else { 0 }) | (if m & T1000_FULL != 0 { ADVERTISE_1000FULL } else { 0 })
}

/// `mii_lpa_mod_linkmode_lpa_t`.
fn lpa_to_modes(lpa: u16) -> u32 {
    let mut m = 0;
    if lpa & ADVERTISE_10HALF != 0 { m |= T10_HALF; }
    if lpa & ADVERTISE_10FULL != 0 { m |= T10_FULL; }
    if lpa & ADVERTISE_100HALF != 0 { m |= T100_HALF; }
    if lpa & ADVERTISE_100FULL != 0 { m |= T100_FULL; }
    if lpa & ADVERTISE_PAUSE_CAP != 0 { m |= PAUSE; }
    if lpa & ADVERTISE_PAUSE_ASYM != 0 { m |= ASYM_PAUSE; }
    if lpa & LPA_LPACK != 0 { m |= AUTONEG; }
    m
}

/// `mii_stat1000_mod_linkmode_lpa_t`.
fn stat1000_to_modes(v: u16) -> u32 {
    (if v & LPA_1000HALF != 0 { T1000_HALF } else { 0 }) | (if v & LPA_1000FULL != 0 { T1000_FULL } else { 0 })
}

/// The MAC this PHY is attached to, as phylink describes it to phylib:
/// `macb_mii_probe` with GIGABIT_MODE_AVAILABLE and EEE on RGMII.
const MAC_MODES: u32 = TP | MII | AUTONEG | ASYM_PAUSE | T10_HALF | T10_FULL | T100_HALF | T100_FULL
    | T1000_HALF | T1000_FULL | X1000_FULL;
const MAC_LPI_MODES: u32 = T100_FULL | T1000_FULL;

/// `phy_state`, the states this board passes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Up,
    NoLink,
    Running,
    Halted,
}

/// What phylink hands the MAC after a state machine run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Up(Link),
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    /// 10, 100 or 1000.
    pub speed: u32,
    pub full_duplex: bool,
    pub tx_pause: bool,
    pub rx_pause: bool,
    /// The MAC may signal LPI (`phydev->enable_tx_lpi`).
    pub tx_lpi: bool,
}

/// A probed BCM54213PE and phylib's state for it.
#[derive(Clone, Copy, Debug)]
pub struct Phy {
    pub state: State,
    pub link: bool,
    autoneg_complete: bool,
    pub speed: u32,
    pub full_duplex: bool,
    pause: bool,
    asym_pause: bool,
    pub supported: u32,
    pub advertising: u32,
    lp_advertising: u32,
    pub supported_eee: u32,
    pub advertising_eee: u32,
    eee_enabled: bool,
    tx_lpi_enabled: bool,
    pub eee_active: bool,
    /// `PHY_BRCM_AUTO_PWRDWN_ENABLE`, from `brcm,powerdown-enable`.
    auto_pwrdwn: bool,
    /// The PHY may stop its TX clock in LPI (`mac_tx_clk_stop`).
    pub tx_clk_stop: bool,
}

/// `__bcm_phy_read_exp`.
fn read_exp(m: &mut impl Mdio, reg: u16) -> Result<u16, Error> {
    m.write(MII_BCM54XX_EXP_SEL, reg)?;
    let v = m.read(MII_BCM54XX_EXP_DATA);
    let _ = m.write(MII_BCM54XX_EXP_SEL, 0);
    v
}

/// `__bcm_phy_write_exp`.
fn write_exp(m: &mut impl Mdio, reg: u16, v: u16) -> Result<(), Error> {
    m.write(MII_BCM54XX_EXP_SEL, reg)?;
    m.write(MII_BCM54XX_EXP_DATA, v)
}

/// `__bcm_phy_modify_exp`.
fn modify_exp(m: &mut impl Mdio, reg: u16, mask: u16, set: u16) -> Result<(), Error> {
    m.write(MII_BCM54XX_EXP_SEL, reg)?;
    let v = m.read(MII_BCM54XX_EXP_DATA)?;
    let new = (v & !mask) | set;
    if new == v { return Ok(()); }
    m.write(MII_BCM54XX_EXP_DATA, new)
}

/// `bcm54xx_auxctl_read`.
fn auxctl_read(m: &mut impl Mdio, regnum: u16) -> Result<u16, Error> {
    m.write(MII_BCM54XX_AUX_CTL, AUXCTL_SHDWSEL_MASK | regnum << AUXCTL_SHDWSEL_READ_SHIFT)?;
    m.read(MII_BCM54XX_AUX_CTL)
}

/// `bcm54xx_auxctl_write`.
fn auxctl_write(m: &mut impl Mdio, regnum: u16, v: u16) -> Result<(), Error> {
    m.write(MII_BCM54XX_AUX_CTL, regnum | v)
}

/// `bcm_phy_read_shadow`.
fn read_shadow(m: &mut impl Mdio, shadow: u16) -> Result<u16, Error> {
    m.write(MII_BCM54XX_SHD, (shadow & 0x1f) << 10)?;
    Ok(m.read(MII_BCM54XX_SHD)? & 0x3ff)
}

/// `bcm_phy_write_shadow`.
fn write_shadow(m: &mut impl Mdio, shadow: u16, v: u16) -> Result<(), Error> {
    m.write(MII_BCM54XX_SHD, MII_BCM54XX_SHD_WRITE | (shadow & 0x1f) << 10 | (v & 0x3ff))
}

/// `__phy_modify_changed`: true when it wrote.
fn modify_changed(m: &mut impl Mdio, reg: u32, mask: u16, set: u16) -> Result<bool, Error> {
    let v = m.read(reg)?;
    let new = (v & !mask) | set;
    if new == v { return Ok(false); }
    m.write(reg, new)?;
    Ok(true)
}

/// `mmd_phy_indirect`.
fn mmd_indirect(m: &mut impl Mdio, devad: u16, reg: u16) -> Result<(), Error> {
    m.write(MII_MMD_CTRL, devad)?;
    m.write(MII_MMD_DATA, reg)?;
    m.write(MII_MMD_CTRL, devad | MII_MMD_CTRL_NOINCR)
}

/// `phy_read_mmd` over Clause 22.
fn read_mmd(m: &mut impl Mdio, devad: u16, reg: u16) -> Result<u16, Error> {
    mmd_indirect(m, devad, reg)?;
    m.read(MII_MMD_DATA)
}

/// `phy_write_mmd` over Clause 22.
fn write_mmd(m: &mut impl Mdio, devad: u16, reg: u16, v: u16) -> Result<(), Error> {
    mmd_indirect(m, devad, reg)?;
    m.write(MII_MMD_DATA, v)
}

/// `phy_modify_mmd_changed`.
fn modify_mmd_changed(m: &mut impl Mdio, devad: u16, reg: u16, mask: u16, set: u16) -> Result<bool, Error> {
    let v = read_mmd(m, devad, reg)?;
    let new = (v & !mask) | set;
    if new == v { return Ok(false); }
    write_mmd(m, devad, reg, new)?;
    Ok(true)
}

/// `get_phy_c22_id`.
pub fn read_id(m: &mut impl Mdio) -> Result<u32, Error> {
    let hi = m.read(MII_PHYSID1)? as u32;
    let lo = m.read(MII_PHYSID2)? as u32;
    Ok(hi << 16 | lo)
}

/// `phy_probe` for a BCM54213PE (`bcm54xx_phy_probe` makes no access):
/// interrupts off, abilities, EEE support and advertisement.
pub fn probe(m: &mut impl Mdio) -> Result<Phy, Error> {
    let mut phy = Phy {
        state: State::Ready, link: false, autoneg_complete: false, speed: 0, full_duplex: false,
        pause: false, asym_pause: false, supported: 0, advertising: 0, lp_advertising: 0,
        supported_eee: 0, advertising_eee: 0, eee_enabled: false, tx_lpi_enabled: false,
        eee_active: false, auto_pwrdwn: false, tx_clk_stop: false,
    };
    phy.config_intr(m)?;
    phy.read_abilities(m)?;
    phy.advertising = phy.supported;
    // genphy_c45_read_eee_adv
    if phy.supported_eee & EEE_CAP1 != 0 {
        let v = read_mmd(m, MDIO_MMD_AN, MDIO_AN_EEE_ADV)?;
        phy.advertising_eee = eee_cap1_to_modes(v);
    }
    phy.advertising_eee &= phy.supported_eee;
    phy.eee_enabled = phy.advertising_eee != 0;
    if phy.supported & (PAUSE | ASYM_PAUSE) == 0 {
        phy.supported |= PAUSE | ASYM_PAUSE;
    }
    Ok(phy)
}

impl Phy {
    /// `genphy_read_abilities` with `genphy_c45_read_eee_abilities`.
    fn read_abilities(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        self.supported |= TP | MII;
        let bmsr = m.read(MII_BMSR)?;
        let set = |s: &mut u32, mode: u32, on: bool| if on { *s |= mode } else { *s &= !mode };
        set(&mut self.supported, AUTONEG, bmsr & BMSR_ANEGCAPABLE != 0);
        set(&mut self.supported, T100_FULL, bmsr & BMSR_100FULL != 0);
        set(&mut self.supported, T100_HALF, bmsr & BMSR_100HALF != 0);
        set(&mut self.supported, T10_FULL, bmsr & BMSR_10FULL != 0);
        set(&mut self.supported, T10_HALF, bmsr & BMSR_10HALF != 0);
        if bmsr & BMSR_ESTATEN != 0 {
            let es = m.read(MII_ESTATUS)?;
            set(&mut self.supported, T1000_FULL, es & ESTATUS_1000_TFULL != 0);
            set(&mut self.supported, T1000_HALF, es & ESTATUS_1000_THALF != 0);
            set(&mut self.supported, X1000_FULL, es & ESTATUS_1000_XFULL != 0);
        }
        if self.supported & EEE_CAP1 != 0 {
            let v = read_mmd(m, MDIO_MMD_PCS, MDIO_PCS_EEE_ABLE)?;
            if v != 0xffff {
                self.supported_eee = eee_cap1_to_modes(v) & self.supported;
            }
        }
        Ok(())
    }

    /// `bcm_phy_config_intr` with interrupts disabled (the PHY is polled).
    fn config_intr(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let ecr = m.read(MII_BCM54XX_ECR)?;
        m.write(MII_BCM54XX_ECR, ecr | MII_BCM54XX_ECR_IM)?;
        m.read(MII_BCM54XX_ISR)?;
        Ok(())
    }

    /// `bcm54xx_adjust_rxrefclk`: CLK125 stays on (no
    /// `PHY_BRCM_RX_REFCLK_UNUSED`), auto power-down per the DT.
    fn adjust_rxrefclk(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let orig = read_shadow(m, BCM54XX_SHD_SCR3)?;
        let mut val = orig;
        if self.auto_pwrdwn {
            val &= !BCM54XX_SHD_SCR3_DLLAPD_DIS;
        } else {
            val |= BCM54XX_SHD_SCR3_DLLAPD_DIS;
        }
        if orig != val {
            write_shadow(m, BCM54XX_SHD_SCR3, val)?;
        }
        let orig = read_shadow(m, BCM54XX_SHD_APD)?;
        let val = if self.auto_pwrdwn { orig | BCM54XX_SHD_APD_EN } else { orig & !BCM54XX_SHD_APD_EN };
        if orig != val {
            write_shadow(m, BCM54XX_SHD_APD, val)?;
        }
        Ok(())
    }

    /// `bcm54xx_config_clock_delay` for `rgmii-id`: RX skew and TX clock
    /// delay both on.
    fn config_clock_delay(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let mut val = auxctl_read(m, AUXCTL_SHDWSEL_MISC)?;
        val |= AUXCTL_MISC_WREN | AUXCTL_SHDWSEL_MISC_RGMII_SKEW_EN;
        auxctl_write(m, AUXCTL_SHDWSEL_MISC, val)?;
        let val = read_shadow(m, BCM54810_SHD_CLK_CTL)? | BCM54810_SHD_CLK_CTL_GTXCLK_EN;
        write_shadow(m, BCM54810_SHD_CLK_CTL, val)
    }

    /// `bcm54xx_phydsp_config`: the SMDSP clock on and off again.
    fn phydsp_config(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let err = auxctl_write(m, AUXCTL_SHDWSEL_AUXCTL, AUXCTL_ACTL_SMDSP_ENA | AUXCTL_ACTL_TX_6DB);
        let err2 = auxctl_write(m, AUXCTL_SHDWSEL_AUXCTL, AUXCTL_ACTL_TX_6DB);
        err.and(err2)
    }

    /// `bcm54xx_config_init` for the BCM54213PE with this board's DT:
    /// `brcm,powerdown-enable`, `led-modes = <0 8>`.
    fn config_init(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        m.read(MII_BCM54XX_ECR)?;
        m.write(MII_BCM54XX_IMR, !(MII_BCM54XX_INT_DUPLEX | MII_BCM54XX_INT_SPEED | MII_BCM54XX_INT_LINK))?;
        self.auto_pwrdwn = true;
        modify_exp(m, BCM54XX_TOP_MISC_MII_BUF_CNTL0, BCM54XX_MII_BUF_CNTL0_AUTOGREEEN_EN, 0)?;
        self.adjust_rxrefclk(m)?;
        // bcm54213pe_config_init -> bcm54210e_config_init, no master mode
        let _ = self.config_clock_delay(m);
        let _ = self.phydsp_config(m);

        const LED_MODES: [u16; 2] = [0x0, 0x8];
        let _ = write_shadow(m, BCM54XX_SHD_LEDS1, BCM_LED_SRC_MULTICOLOR1 | BCM_LED_SRC_MULTICOLOR1 << 4);
        let reg = read_shadow(m, BCM54XX_SHD_LEDS2).unwrap_or(0);
        let _ = write_shadow(m, BCM54XX_SHD_LEDS2, (reg & !0xf) | BCM_LED_SRC_MULTICOLOR1);
        let _ = write_exp(m, BCM_EXP_MULTICOLOR,
            BCM_LED_MULTICOLOR_IN_PHASE | (LED_MODES[0] & 0xf) | (LED_MODES[1] & 0xf) << 4);

        read_exp(m, BCM54XX_WOL_INT_STATUS)?;
        Ok(())
    }

    /// `bcm54xx_resume` (no WoL, no IDDQ): `genphy_resume`, 40 us in
    /// internal reset, `config_init`.
    fn resume(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        modify_changed(m, MII_BMCR, BMCR_PDOWN, 0)?;
        m.delay_us(40);
        self.config_init(m)
    }

    /// `bcm54xx_suspend` (no WoL): power down without read-modify-write.
    fn suspend(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        read_exp(m, BCM54XX_WOL_INT_STATUS)?;
        m.write(MII_BMCR, BMCR_PDOWN)
    }

    /// `bcm54xx_link_change_notify`: early DAC wake only at 10 Mb/s.
    fn link_change_notify(&mut self, m: &mut impl Mdio) {
        if self.state != State::Running || !self.auto_pwrdwn { return; }
        let Ok(v) = read_exp(m, MII_BCM54XX_EXP_EXP08) else { return };
        let v = if self.speed == 10 { v | EXP08_DAC_WAKE } else { v & !EXP08_DAC_WAKE };
        let _ = write_exp(m, MII_BCM54XX_EXP_EXP08, v);
    }

    /// `phylink_of_phy_connect`: `phy_attach_direct` (`phy_init_hw`,
    /// `phy_resume`), then `phylink_bringup_phy`'s restriction to the MAC's
    /// modes, EEE through the MAC (`mac_supports_eee`), and its clock-stop
    /// accesses (the MAC does not let the PHY stop the RX clock).
    pub fn attach(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        self.config_init(m)?;
        self.config_intr(m)?;
        self.resume(m)?;

        self.advertising = (self.advertising & !(PAUSE | ASYM_PAUSE)) | (self.supported & (PAUSE | ASYM_PAUSE));
        self.supported &= MAC_MODES;
        self.advertising &= MAC_MODES;
        self.advertising_eee = self.supported_eee;
        self.tx_lpi_enabled = true;
        self.eee_enabled = true;
        self.advertising_eee &= MAC_LPI_MODES;

        self.tx_clk_stop = read_mmd(m, MDIO_MMD_PCS, MDIO_STAT1)? & MDIO_PCS_STAT1_CLKSTOP_CAP != 0;
        modify_mmd_changed(m, MDIO_MMD_PCS, MDIO_CTRL1, MDIO_PCS_CTRL1_CLKSTOP_EN, 0)?;
        Ok(())
    }

    /// `phy_start`: resume, then the state machine starts in UP. The first
    /// [`Phy::poll`] is due at once, then one every second (`PHY_STATE_TIME`).
    pub fn start(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        self.resume(m)?;
        self.state = State::Up;
        Ok(())
    }

    /// `genphy_c45_an_config_eee_aneg`.
    fn config_eee_aneg(&mut self, m: &mut impl Mdio) -> Result<bool, Error> {
        if self.supported_eee & EEE_CAP1 == 0 { return Ok(false); }
        let adv = if self.eee_enabled { self.advertising_eee } else { 0 };
        modify_mmd_changed(m, MDIO_MMD_AN, MDIO_AN_EEE_ADV, MDIO_EEE_CAP1_MASK, modes_to_eee_cap1(adv))
    }

    /// `genphy_config_aneg` (`__genphy_config_aneg`), autonegotiation on.
    fn config_aneg(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let mut changed = self.config_eee_aneg(m)?;
        self.advertising &= self.supported;
        changed |= modify_changed(m, MII_ADVERTISE,
            ADVERTISE_ALL | ADVERTISE_100BASE4 | ADVERTISE_PAUSE_CAP | ADVERTISE_PAUSE_ASYM,
            modes_to_adv(self.advertising))?;
        let bmsr = m.read(MII_BMSR)?;
        if bmsr & BMSR_ESTATEN != 0 {
            changed |= modify_changed(m, MII_CTRL1000, ADVERTISE_1000FULL | ADVERTISE_1000HALF,
                modes_to_ctrl1000(self.advertising))?;
        }
        if !changed {
            let bmcr = m.read(MII_BMCR)?;
            changed = bmcr & BMCR_ANENABLE == 0 || bmcr & BMCR_ISOLATE != 0;
        }
        if changed {
            modify_changed(m, MII_BMCR, BMCR_ISOLATE, BMCR_ANENABLE | BMCR_ANRESTART)?;
        }
        Ok(())
    }

    /// `genphy_update_link` in polling mode.
    fn update_link(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let bmcr = m.read(MII_BMCR)?;
        let mut status = 0;
        if bmcr & BMCR_ANRESTART == 0 {
            let mut done = false;
            if !self.link {
                status = m.read(MII_BMSR)?;
                done = status & BMSR_LSTATUS != 0;
            }
            if !done {
                status = m.read(MII_BMSR)?;
            }
        }
        self.link = status & BMSR_LSTATUS != 0;
        self.autoneg_complete = status & BMSR_ANEGCOMPLETE != 0;
        if !self.autoneg_complete {
            self.link = false;
        }
        Ok(())
    }

    /// `genphy_read_status`, autonegotiation on, gigabit capable.
    fn read_status(&mut self, m: &mut impl Mdio) -> Result<(), Error> {
        let old_link = self.link;
        self.update_link(m)?;
        if old_link && self.link { return Ok(()); }
        self.speed = 0;
        self.full_duplex = false;
        self.pause = false;
        self.asym_pause = false;
        // genphy_read_master_slave: the result only feeds ethtool.
        m.read(MII_CTRL1000)?;
        m.read(MII_STAT1000)?;
        // genphy_read_lpa
        if !self.autoneg_complete {
            self.lp_advertising = 0;
            return Ok(());
        }
        let lpagb = m.read(MII_STAT1000)?;
        if lpagb & LPA_1000MSFAIL != 0 {
            let _ = m.read(MII_CTRL1000)? & CTL1000_ENABLE_MASTER;
            return Err(Error::MasterSlave);
        }
        let lpa = m.read(MII_LPA)?;
        self.lp_advertising = stat1000_to_modes(lpagb) | lpa_to_modes(lpa);
        // phy_resolve_aneg_linkmode
        let common = self.advertising & self.lp_advertising;
        (self.speed, self.full_duplex) = [
            (T1000_FULL, 1000, true), (T1000_HALF, 1000, false), (T100_FULL, 100, true),
            (T100_HALF, 100, false), (T10_FULL, 10, true), (T10_HALF, 10, false),
        ].iter().find(|c| common & c.0 != 0).map(|c| (c.1, c.2)).unwrap_or((0, false));
        if self.full_duplex {
            self.pause = self.lp_advertising & PAUSE != 0;
            self.asym_pause = self.lp_advertising & ASYM_PAUSE != 0;
        }
        Ok(())
    }

    /// `genphy_c45_eee_is_active` at the resolved speed and duplex.
    fn eee_is_active(&mut self, m: &mut impl Mdio) -> Result<bool, Error> {
        if !self.eee_enabled { return Ok(false); }
        let lp = if self.supported_eee & EEE_CAP1 != 0 {
            eee_cap1_to_modes(read_mmd(m, MDIO_MMD_AN, MDIO_AN_EEE_LPABLE)?)
        } else { 0 };
        let common = self.advertising_eee & lp;
        let mode = match (self.speed, self.full_duplex) {
            (1000, true) => T1000_FULL,
            (100, true) => T100_FULL,
            _ => 0,
        };
        Ok(common & mode != 0)
    }

    /// `phy_check_link_status`.
    fn check_link_status(&mut self, m: &mut impl Mdio) -> Result<Option<Event>, Error> {
        self.read_status(m)?;
        if self.link && self.state != State::Running {
            self.state = State::Running;
            self.eee_active = self.eee_is_active(m).unwrap_or(false);
            return Ok(Some(Event::Up(self.resolve())));
        }
        if !self.link && self.state != State::NoLink {
            self.state = State::NoLink;
            self.eee_active = false;
            return Ok(Some(Event::Down));
        }
        Ok(None)
    }

    /// phylink's view of the link (`phylink_resolve` with `MLO_PAUSE_AN`):
    /// pause from `linkmode_resolve_pause`.
    fn resolve(&self) -> Link {
        let local = self.advertising & (PAUSE | ASYM_PAUSE);
        let mut partner = 0;
        if self.pause { partner |= PAUSE; }
        if self.asym_pause { partner |= ASYM_PAUSE; }
        let common = local & partner;
        let (tx_pause, rx_pause) = if common & PAUSE != 0 {
            (true, true)
        } else if common & ASYM_PAUSE != 0 {
            (partner & PAUSE != 0, local & PAUSE != 0)
        } else {
            (false, false)
        };
        Link { speed: self.speed, full_duplex: self.full_duplex, tx_pause, rx_pause,
            tx_lpi: self.tx_lpi_enabled && self.eee_active }
    }

    /// One run of `phy_state_machine`; [`Event`] is what phylink's resolve
    /// then hands the MAC.
    pub fn poll(&mut self, m: &mut impl Mdio) -> Result<Option<Event>, Error> {
        let old = self.state;
        let ev = match self.state {
            State::Up => {
                self.config_aneg(m)?;
                self.check_link_status(m)?
            }
            State::NoLink | State::Running => self.check_link_status(m)?,
            State::Ready | State::Halted => None,
        };
        if self.state != old {
            self.link_change_notify(m);
        }
        Ok(ev)
    }

    /// `phy_stop` (from `phylink_stop`): HALTED, link down, suspend.
    pub fn stop(&mut self, m: &mut impl Mdio) -> Result<Option<Event>, Error> {
        let old = self.state;
        self.state = State::Halted;
        if old != State::Halted {
            self.link_change_notify(m);
        }
        let ev = if self.link {
            self.link = false;
            self.eee_active = false;
            Some(Event::Down)
        } else { None };
        self.suspend(m)?;
        Ok(ev)
    }
}
