//! RP1's clock manager, after Linux `drivers/clk/clk-rp1.c`: the share of
//! the clock tree the Ethernet MAC runs on. PLL_SYS (core, primary and
//! secondary output), CLK_SYS (the MAC's pclk and hclk), CLK_ETH (tx_clk,
//! from PLL_SYS_SEC) and CLK_ETH_TSU (from the crystal).
//!
//! The ops repeat clk-rp1's register access. [`eth_clocks`] stands in for
//! the clock core on this share: the critical clocks are switched on as at
//! registration, then the node's `assigned-clock-rates` are applied in DT
//! order, each only when the clock is not already at that rate and parent
//! (the clock core's `clk_set_rate` / `clk_set_parent` decisions).
//! [`macb_clk_init`] is the MAC driver's enables.

use crate::hw::{self, Block, Hw};

/// `clocks@18000` in BAR1.
pub const CLOCKS_BASE: u64 = 0x1_8000;
/// `clk_xosc`, the parent of the PLLs and of CLK_ETH_TSU.
pub const XOSC_HZ: u64 = 50_000_000;

const PLL_CS_LOCK: u32 = 1 << 31;
const PLL_CS_REFDIV_UNITY: u32 = 1;
const PLL_PWR_DSMPD: u32 = 1 << 2;
const PLL_PWR_MASK: u32 = 0x3f;
const PLL_PRIM_DIV1_MASK: u32 = 0x7 << 16;
const PLL_PRIM_DIV2_MASK: u32 = 0x7 << 12;
const PLL_SEC_DIV_MASK: u32 = 0x1f << 8;
const PLL_SEC_RST: u32 = 1 << 16;
const PLL_SEC_IMPL: u32 = 1 << 31;
const PLL_DIV_INVALID: u32 = 19;
const CLK_CTRL_ENABLE: u32 = 1 << 11;
const CLK_CTRL_AUXSRC_MASK: u32 = 0x1f << 5;
const CLK_DIV_FRAC_BITS: u32 = 16;
const AUX_SEL: u32 = 1;
const LOCK_TIMEOUT_US: u64 = 100_000;
const LOCK_POLL_DELAY_US: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// `can't lock PLL`.
    Lock,
    /// Reference above VCO / 16 (`rp1_pll_core_set_rate`'s WARN).
    Range,
    /// The parent did not take (`rp1_clock_set_parent`).
    Parent,
}

fn rd(hw: &mut impl Hw, reg: u32) -> u32 { hw.r32(Block::Rp1Bar1, CLOCKS_BASE + reg as u64) }
fn wr(hw: &mut impl Hw, reg: u32, v: u32) { hw.w32(Block::Rp1Bar1, CLOCKS_BASE + reg as u64, v) }

fn div_round_closest(x: u64, d: u64) -> u64 { (x + d / 2) / d }

/// `rp1_pll_core_data`.
pub struct PllCore {
    pub cs: u32,
    pub pwr: u32,
    pub fbdiv_int: u32,
    pub fbdiv_frac: u32,
}

/// `rp1_pll_data` of a primary output (`rp1_pll_ops`).
pub struct Pll {
    pub ctrl: u32,
}

/// `rp1_pll_data` of a secondary output (`rp1_pll_divider_ops`).
pub struct PllDivider {
    pub ctrl: u32,
}

/// `rp1_clock_data`.
pub struct Clock {
    pub num_std_parents: u32,
    pub num_aux_parents: u32,
    pub ctrl: u32,
    pub div_int: u32,
    pub div_frac: u32,
    pub sel: u32,
    pub div_int_max: u32,
    pub clk_src_mask: u32,
    pub oe_mask: u32,
}

pub const PLL_SYS_CORE: PllCore = PllCore { cs: 0x8000, pwr: 0x8004, fbdiv_int: 0x8008, fbdiv_frac: 0x800c };
pub const PLL_SYS: Pll = Pll { ctrl: 0x8010 };
pub const PLL_SYS_SEC: PllDivider = PllDivider { ctrl: 0x8014 };
pub const CLK_SYS: Clock = Clock {
    num_std_parents: 3, num_aux_parents: 0, ctrl: 0x14, div_int: 0x18, div_frac: 0, sel: 0x20,
    div_int_max: 0xff_ffff, clk_src_mask: 0x3, oe_mask: 0,
};
pub const CLK_ETH: Clock = Clock {
    num_std_parents: 0, num_aux_parents: 3, ctrl: 0x64, div_int: 0x68, div_frac: 0, sel: 0x70,
    div_int_max: 0xff, clk_src_mask: 0, oe_mask: 0,
};
/// `clk_pwm1`: aux parents none, `pll_video_sec`, `xosc`.
pub const CLK_PWM1: Clock = Clock {
    num_std_parents: 0, num_aux_parents: 3, ctrl: 0x84, div_int: 0x88, div_frac: 0x8c, sel: 0x90,
    div_int_max: 0xffff, clk_src_mask: 0, oe_mask: 0,
};
pub const CLK_ETH_TSU: Clock = Clock {
    num_std_parents: 0, num_aux_parents: 8, ctrl: 0x134, div_int: 0x138, div_frac: 0, sel: 0x140,
    div_int_max: 0xff, clk_src_mask: 0, oe_mask: 0,
};

/// `get_pll_core_divider`: (rate, FBDIV_INT, FBDIV_FRAC).
fn pll_core_divider(rate: u64, parent_rate: u64) -> (u64, u32, u32) {
    let mut div_fp64 = (((rate as u128) << 32) + parent_rate as u128 / 2) / parent_rate as u128;
    div_fp64 += 1 << (32 - 24 - 1);
    let int = (div_fp64 >> 32) as u32;
    let frac = ((div_fp64 >> (32 - 24)) & 0xff_ffff) as u32;
    (pll_core_rate(parent_rate, int, frac), int, frac)
}

fn pll_core_rate(parent_rate: u64, int: u32, frac: u32) -> u64 {
    let calc = parent_rate as u128 * (((int as u128) << 24) + frac as u128) + (1 << 23);
    (calc >> 24) as u64
}

impl PllCore {
    /// `rp1_pll_core_on`.
    pub fn on(&self, hw: &mut impl Hw) -> Result<(), Error> {
        if rd(hw, self.cs) & PLL_CS_LOCK == 0 {
            wr(hw, self.pwr, PLL_PWR_MASK);
            wr(hw, self.fbdiv_int, 20);
            wr(hw, self.fbdiv_frac, 0);
            wr(hw, self.cs, PLL_CS_REFDIV_UNITY);
        }
        let frac = rd(hw, self.fbdiv_frac);
        wr(hw, self.pwr, if frac != 0 { 0 } else { PLL_PWR_DSMPD });
        let (ok, _) = hw::poll(hw, Block::Rp1Bar1, CLOCKS_BASE + self.cs as u64, LOCK_POLL_DELAY_US, LOCK_TIMEOUT_US,
            |v| v & PLL_CS_LOCK != 0);
        if ok { Ok(()) } else { Err(Error::Lock) }
    }

    /// `rp1_pll_core_set_rate`.
    pub fn set_rate(&self, hw: &mut impl Hw, rate: u64, parent_rate: u64) -> Result<(), Error> {
        wr(hw, self.fbdiv_int, 0);
        wr(hw, self.fbdiv_frac, 0);
        let (_, int, frac) = pll_core_divider(rate, parent_rate);
        wr(hw, self.pwr, if frac != 0 { 0 } else { PLL_PWR_DSMPD });
        wr(hw, self.fbdiv_int, int);
        wr(hw, self.fbdiv_frac, frac);
        if parent_rate > rate / 16 {
            return Err(Error::Range);
        }
        let cs = rd(hw, self.cs);
        wr(hw, self.cs, cs | PLL_CS_REFDIV_UNITY);
        Ok(())
    }

    /// `rp1_pll_core_recalc_rate`.
    pub fn recalc_rate(&self, hw: &mut impl Hw, parent_rate: u64) -> u64 {
        let int = rd(hw, self.fbdiv_int);
        let frac = rd(hw, self.fbdiv_frac);
        pll_core_rate(parent_rate, int, frac)
    }
}

/// `get_pll_prim_dividers`.
fn pll_prim_dividers(rate: u64, parent_rate: u64) -> (u32, u32) {
    let (mut best1, mut best2) = (7u32, 7u32);
    let mut best_diff = div_round_closest(parent_rate, 49).abs_diff(rate);
    for d1 in 1..=7u32 {
        for d2 in 1..=d1 {
            let calc = div_round_closest(parent_rate, (d1 * d2) as u64);
            let diff = calc.abs_diff(rate);
            if calc == rate {
                return (d1, d2);
            } else if diff < best_diff {
                (best1, best2, best_diff) = (d1, d2, diff);
            }
        }
    }
    (best1, best2)
}

impl Pll {
    /// `rp1_pll_set_rate`.
    pub fn set_rate(&self, hw: &mut impl Hw, rate: u64, parent_rate: u64) {
        let (d1, d2) = pll_prim_dividers(rate, parent_rate);
        let mut prim = rd(hw, self.ctrl);
        hw::replace_bits(&mut prim, d1, PLL_PRIM_DIV1_MASK);
        hw::replace_bits(&mut prim, d2, PLL_PRIM_DIV2_MASK);
        wr(hw, self.ctrl, prim);
    }

    /// `rp1_pll_recalc_rate`: 0 for a zero divider.
    pub fn recalc_rate(&self, hw: &mut impl Hw, parent_rate: u64) -> u64 {
        let prim = rd(hw, self.ctrl);
        let (d1, d2) = (hw::field_get(prim, PLL_PRIM_DIV1_MASK), hw::field_get(prim, PLL_PRIM_DIV2_MASK));
        if d1 == 0 || d2 == 0 { return 0; }
        div_round_closest(parent_rate, (d1 * d2) as u64)
    }
}

impl PllDivider {
    /// `rp1_pll_divider_on`. Linux warns, and goes on, when IMPL reads 0.
    pub fn on(&self, hw: &mut impl Hw) -> bool {
        let implemented = rd(hw, self.ctrl) & PLL_SEC_IMPL != 0;
        let v = rd(hw, self.ctrl);
        wr(hw, self.ctrl, v & !PLL_SEC_RST);
        implemented
    }

    /// `rp1_pll_divider_set_rate`.
    pub fn set_rate(&self, hw: &mut impl Hw, rate: u64, parent_rate: u64) {
        let div = (parent_rate.div_ceil(rate) as u32).clamp(8, 19);
        let mut sec = rd(hw, self.ctrl);
        hw::replace_bits(&mut sec, div, PLL_SEC_DIV_MASK);
        sec |= PLL_SEC_RST;
        wr(hw, self.ctrl, sec);
        let ns = 10 * div as u64 * 1_000_000_000 / parent_rate;
        hw.delay_us(ns.div_ceil(1000));
        sec &= !PLL_SEC_RST;
        wr(hw, self.ctrl, sec);
    }

    /// `clk_divider_ops.recalc_rate` over `pll_sec_div_table`.
    pub fn recalc_rate(&self, hw: &mut impl Hw, parent_rate: u64) -> u64 {
        let val = hw::field_get(rd(hw, self.ctrl), PLL_SEC_DIV_MASK);
        let div = if (8..=18).contains(&val) { val } else { PLL_DIV_INVALID };
        parent_rate.div_ceil(div as u64)
    }
}

impl Clock {
    /// `rp1_clock_on`.
    pub fn on(&self, hw: &mut impl Hw) {
        let v = rd(hw, self.ctrl);
        wr(hw, self.ctrl, v | CLK_CTRL_ENABLE);
        if self.oe_mask != 0 {
            let oe = rd(hw, 0);
            wr(hw, 0, oe | self.oe_mask);
        }
    }

    /// `rp1_clock_choose_div`: 16.16 fixed point.
    fn choose_div(&self, rate: u64, parent_rate: u64) -> u64 {
        if rate == 0 || rate > parent_rate + (parent_rate >> CLK_DIV_FRAC_BITS) {
            return 0;
        }
        let div = if self.div_frac != 0 {
            div_round_closest(parent_rate << CLK_DIV_FRAC_BITS, rate)
        } else {
            div_round_closest(parent_rate, rate) << CLK_DIV_FRAC_BITS
        };
        div.clamp(1 << CLK_DIV_FRAC_BITS, (self.div_int_max as u64) << CLK_DIV_FRAC_BITS)
    }

    /// `rp1_clock_get_parent`.
    pub fn get_parent(&self, hw: &mut impl Hw) -> u32 {
        let sel = rd(hw, self.sel);
        let mut parent = sel.trailing_zeros();
        if sel == 0 {
            parent = rd(hw, self.ctrl) & self.clk_src_mask;
        }
        if parent >= self.num_std_parents {
            parent = AUX_SEL;
        }
        if parent == AUX_SEL {
            let ctrl = rd(hw, self.ctrl);
            parent = hw::field_get(ctrl, CLK_CTRL_AUXSRC_MASK) + self.num_std_parents;
        }
        parent
    }

    /// `rp1_clock_set_parent`.
    pub fn set_parent(&self, hw: &mut impl Hw, index: u32) -> Result<(), Error> {
        let mut ctrl = rd(hw, self.ctrl);
        if index >= self.num_std_parents {
            if index >= self.num_std_parents + self.num_aux_parents {
                return Err(Error::Parent);
            }
            hw::replace_bits(&mut ctrl, index - self.num_std_parents, CLK_CTRL_AUXSRC_MASK);
            ctrl = (ctrl & !self.clk_src_mask) | (AUX_SEL & self.clk_src_mask);
        } else {
            ctrl = (ctrl & !self.clk_src_mask) | (index & self.clk_src_mask);
        }
        wr(hw, self.ctrl, ctrl);
        if self.get_parent(hw) != index { Err(Error::Parent) } else { Ok(()) }
    }

    /// `rp1_clock_set_rate_and_parent`; `rp1_clock_set_rate` with `None`.
    pub fn set_rate_and_parent(&self, hw: &mut impl Hw, rate: u64, parent_rate: u64, parent: Option<u32>) -> Result<(), Error> {
        let div = self.choose_div(rate, parent_rate) as u32;
        wr(hw, self.div_int, div >> CLK_DIV_FRAC_BITS);
        if self.div_frac != 0 {
            wr(hw, self.div_frac, div << (32 - CLK_DIV_FRAC_BITS));
        }
        match parent {
            Some(p) => self.set_parent(hw, p),
            None => Ok(()),
        }
    }

    /// `rp1_clock_recalc_rate`.
    pub fn recalc_rate(&self, hw: &mut impl Hw, parent_rate: u64) -> u64 {
        let mut div = rd(hw, self.div_int) as u64;
        let frac = if self.div_frac != 0 { rd(hw, self.div_frac) } else { 0 };
        if div == 0 { div = 1 << 16; }
        let div = (div << CLK_DIV_FRAC_BITS) | (frac >> (32 - CLK_DIV_FRAC_BITS)) as u64;
        ((parent_rate as u128) << CLK_DIV_FRAC_BITS).checked_div(div as u128).unwrap_or(0) as u64
    }

    /// `clk_set_rate` with the parent the clock core's `determine_rate`
    /// picks on this board: reparent and divide, or only divide, or
    /// nothing when both already hold.
    fn assign(&self, hw: &mut impl Hw, rate: u64, parent: u32, parent_rate: u64) -> Result<(), Error> {
        if self.get_parent(hw) != parent {
            return self.set_rate_and_parent(hw, rate, parent_rate, Some(parent));
        }
        if self.recalc_rate(hw, parent_rate) != rate {
            self.set_rate_and_parent(hw, rate, parent_rate, None)?;
        }
        Ok(())
    }
}

/// The rates the MAC's clocks run at once [`eth_clocks`] and
/// [`macb_clk_init`] ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EthRates {
    /// pclk and hclk.
    pub sys: u64,
    /// tx_clk.
    pub eth: u64,
    pub tsu: u64,
}

pub const ETH_RATES: EthRates = EthRates { sys: CLK_SYS_HZ, eth: CLK_ETH_HZ, tsu: CLK_ETH_TSU_HZ };

/// `rp1_clocks`' assigned rates on the Ethernet share.
const PLL_SYS_CORE_HZ: u64 = 1_000_000_000;
const PLL_SYS_HZ: u64 = 200_000_000;
const PLL_SYS_SEC_HZ: u64 = 125_000_000;
const CLK_ETH_HZ: u64 = 125_000_000;
const CLK_SYS_HZ: u64 = 200_000_000;
const CLK_ETH_TSU_HZ: u64 = 50_000_000;
/// CLK_SYS's parent `pll_sys`; CLK_ETH's `pll_sys_sec` and CLK_ETH_TSU's
/// `xosc` are index 0.
const CLK_SYS_PARENT_PLL_SYS: u32 = 2;

/// `rp1_clk_probe` on the Ethernet share: registration of the critical
/// clocks, then the assigned rates.
pub fn eth_clocks(hw: &mut impl Hw) -> Result<(), Error> {
    // Registration caches the core's rate before switching it on.
    let core_cached = PLL_SYS_CORE.recalc_rate(hw, XOSC_HZ);
    PLL_SYS_CORE.on(hw)?;
    CLK_SYS.on(hw);

    if core_cached != PLL_SYS_CORE_HZ {
        PLL_SYS_CORE.set_rate(hw, PLL_SYS_CORE_HZ, XOSC_HZ)?;
    }
    if PLL_SYS.recalc_rate(hw, PLL_SYS_CORE_HZ) != PLL_SYS_HZ {
        PLL_SYS.set_rate(hw, PLL_SYS_HZ, PLL_SYS_CORE_HZ);
    }
    if PLL_SYS_SEC.recalc_rate(hw, PLL_SYS_CORE_HZ) != PLL_SYS_SEC_HZ {
        PLL_SYS_SEC.set_rate(hw, PLL_SYS_SEC_HZ, PLL_SYS_CORE_HZ);
    }
    CLK_ETH.assign(hw, CLK_ETH_HZ, 0, PLL_SYS_SEC_HZ)?;
    CLK_SYS.assign(hw, CLK_SYS_HZ, CLK_SYS_PARENT_PLL_SYS, PLL_SYS_HZ)?;
    CLK_ETH_TSU.assign(hw, CLK_ETH_TSU_HZ, 0, XOSC_HZ)?;
    Ok(())
}

/// `macb_clk_init`'s `clk_prepare_enable`s: pclk and hclk are the critical
/// CLK_SYS, already on; tx_clk enables its parent first; no rx_clk.
pub fn macb_clk_init(hw: &mut impl Hw) -> EthRates {
    PLL_SYS_SEC.on(hw);
    CLK_ETH.on(hw);
    CLK_ETH_TSU.on(hw);
    ETH_RATES
}

/// `clk_pwm1`'s parent `xosc`.
const CLK_PWM1_PARENT_XOSC: u32 = 2;

/// The PWM1 node's `assigned-clock-rates` and `devm_clk_get_enabled`:
/// `rate` from the crystal (the parent `rp1_clock_determine_rate` picks
/// when it divides exactly), then the clock on. Returns the rate.
pub fn pwm1_clock(hw: &mut impl Hw, rate: u64) -> Result<u64, Error> {
    CLK_PWM1.assign(hw, rate, CLK_PWM1_PARENT_XOSC, XOSC_HZ)?;
    CLK_PWM1.on(hw);
    Ok(CLK_PWM1.recalc_rate(hw, XOSC_HZ))
}
