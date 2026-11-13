//! `drivers/i2c/busses/i2c-brcmstb.c` (Linux rpi-6.18): the BSC
//! controllers the HDMI ports read their monitor's EDID and SCDC through.
//!
//! Both sit on lines 1 and 2 of the `bsc_irq` L2 controller and nothing
//! else does, so [`Bsc`] holds the controller with them: waiting for a
//! transfer runs that controller's chained handler, which completes the
//! transfer. Polling mode, atomic transfers and `bcm2711_release_bsc`
//! (`brcm,bcm2711-hdmi-i2c`, not on the Pi 5) are not ported.

use crate::hw::{Block, Hw, Spi};
use crate::irq_brcmstb_l2::{IrqReturn, L2Intc};
use crate::linux::{EREMOTEIO, ETIMEDOUT};

const N_DATA_REGS: u32 = 8;

/// `offsetof(struct bsc_regs, ...)`.
const CHIP_ADDRESS: u32 = 0x00;
const DATA_IN: u32 = 0x04;
const CNT_REG: u32 = 0x24;
const CTL_REG: u32 = 0x28;
const IIC_ENABLE: u32 = 0x2c;
const DATA_OUT: u32 = 0x30;
const CTLHI_REG: u32 = 0x50;

fn bsc_cnt_reg1_mask(nb: u32) -> u32 {
    if nb == 1 { 0xf } else { 0x3f }
}
const BSC_CNT_REG1_SHIFT: u32 = 0;

const BSC_CTL_REG_DTF_MASK: u32 = 0x0000_0003;
const BSC_CTL_REG_SCL_SEL_MASK: u32 = 0x0000_0030;
const BSC_CTL_REG_SCL_SEL_SHIFT: u32 = 4;
const BSC_CTL_REG_INT_EN_MASK: u32 = 0x0000_0040;
const BSC_CTL_REG_DIV_CLK_MASK: u32 = 0x0000_0080;

const BSC_IIC_EN_RESTART_MASK: u32 = 0x0000_0040;
const BSC_IIC_EN_NOSTART_MASK: u32 = 0x0000_0020;
const BSC_IIC_EN_NOSTOP_MASK: u32 = 0x0000_0010;
const BSC_IIC_EN_NOACK_MASK: u32 = 0x0000_0004;
const BSC_IIC_EN_INTRP_MASK: u32 = 0x0000_0002;
const BSC_IIC_EN_ENABLE_MASK: u32 = 0x0000_0001;

const BSC_CTLHI_REG_DATAREG_SIZE_MASK: u32 = 0x0000_0040;
const BSC_CTLHI_REG_IGNORE_ACK_MASK: u32 = 0x0000_0002;

/// msecs.
const I2C_TIMEOUT: u32 = 100;

const COND_RESTART: u32 = BSC_IIC_EN_RESTART_MASK;
const COND_NOSTART: u32 = BSC_IIC_EN_NOSTART_MASK;
const COND_NOSTOP: u32 = BSC_IIC_EN_NOSTOP_MASK;
const COND_START_STOP: u32 = COND_RESTART | COND_NOSTART | COND_NOSTOP;

const DTF_WR_MASK: u32 = 0x0000_0000;
const DTF_RD_MASK: u32 = 0x0000_0001;

/// `include/uapi/linux/i2c.h`.
pub const I2C_M_RD: u16 = 0x0001;
pub const I2C_M_TEN: u16 = 0x0010;
pub const I2C_M_IGNORE_NAK: u16 = 0x1000;
pub const I2C_M_NOSTART: u16 = 0x4000;

/// `struct i2c_msg`.
pub struct I2cMsg<'a> {
    pub addr: u16,
    pub flags: u16,
    pub buf: &'a mut [u8],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BscXferCmd {
    Wr,
    Rd,
    WrNoack,
    RdNoack,
}

/// `struct bsc_clk_param`.
struct BscClkParam {
    hz: u32,
    scl_mask: u32,
    div_mask: u32,
}

const SPD_375K: u32 = 0;
const SPD_390K: u32 = 1;
const SPD_187K: u32 = 2;
const SPD_200K: u32 = 3;

const BSC_CLK: [BscClkParam; 8] = [
    BscClkParam { hz: 375000, scl_mask: SPD_375K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: 0 },
    BscClkParam { hz: 390000, scl_mask: SPD_390K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: 0 },
    BscClkParam { hz: 187500, scl_mask: SPD_187K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: 0 },
    BscClkParam { hz: 200000, scl_mask: SPD_200K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: 0 },
    BscClkParam { hz: 93750, scl_mask: SPD_375K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: BSC_CTL_REG_DIV_CLK_MASK },
    BscClkParam { hz: 97500, scl_mask: SPD_390K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: BSC_CTL_REG_DIV_CLK_MASK },
    BscClkParam { hz: 46875, scl_mask: SPD_187K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: BSC_CTL_REG_DIV_CLK_MASK },
    BscClkParam { hz: 50000, scl_mask: SPD_200K << BSC_CTL_REG_SCL_SEL_SHIFT, div_mask: BSC_CTL_REG_DIV_CLK_MASK },
];

/// `struct brcmstb_i2c_dev`; `bsc_regmap` keeps only the fields the
/// driver caches (`ctl_reg`, `iic_enable`, `ctlhi_reg`).
pub struct BrcmstbI2cDev {
    block: Block,
    /// The line on the `bsc_irq` controller.
    irq: u32,
    ctl_reg: u32,
    iic_enable: u32,
    ctlhi_reg: u32,
    done: bool,
    pub clk_freq_hz: u32,
    data_regsz: u32,
}

impl BrcmstbI2cDev {
    /// `brcmstb_i2c_get_xfersz`.
    fn get_xfersz(&self) -> u32 {
        N_DATA_REGS * self.data_regsz
    }

    /// `brcmstb_i2c_enable_disable_irq`.
    fn enable_disable_irq(&mut self, hw: &mut impl Hw, int_en: bool) {
        if int_en {
            self.ctl_reg |= BSC_CTL_REG_INT_EN_MASK;
        } else {
            self.ctl_reg &= !BSC_CTL_REG_INT_EN_MASK;
        }
        hw.write(self.block, CTL_REG, self.ctl_reg);
    }

    /// `brcmstb_i2c_isr`.
    fn isr(&mut self, hw: &mut impl Hw) -> IrqReturn {
        let status_bsc_ctl = hw.read(self.block, CTL_REG);
        let _status_iic_intrp = hw.read(self.block, IIC_ENABLE);
        if status_bsc_ctl & BSC_CTL_REG_INT_EN_MASK == 0 {
            return IrqReturn::None;
        }
        self.enable_disable_irq(hw, false);
        self.done = true;
        IrqReturn::Handled
    }

    /// `brcmstb_i2c_wait_if_busy`.
    fn wait_if_busy(&mut self, hw: &mut impl Hw) -> Result<(), i32> {
        let timeout = hw.now_us() + I2C_TIMEOUT as u64 * 1000;
        while hw.read(self.block, IIC_ENABLE) & BSC_IIC_EN_INTRP_MASK != 0 {
            if hw.now_us() > timeout {
                return Err(-ETIMEDOUT);
            }
        }
        Ok(())
    }

    /// `brcmstb_set_i2c_start_stop`.
    fn set_i2c_start_stop(&mut self, cond_flag: u32) {
        self.iic_enable = (self.iic_enable & !COND_START_STOP) | cond_flag;
    }

    /// `brcmstb_i2c_set_bus_speed`.
    fn set_bus_speed(&mut self, hw: &mut impl Hw) {
        match BSC_CLK.iter().find(|c| c.hz == self.clk_freq_hz) {
            Some(c) => {
                self.ctl_reg &= !(BSC_CTL_REG_SCL_SEL_MASK | BSC_CTL_REG_DIV_CLK_MASK);
                self.ctl_reg |= c.scl_mask | c.div_mask;
                hw.write(self.block, CTL_REG, self.ctl_reg);
            }
            None => {
                let _ = (hw.read(self.block, CTL_REG) & BSC_CTL_REG_SCL_SEL_MASK) >> BSC_CTL_REG_SCL_SEL_SHIFT;
            }
        }
    }

    /// `brcmstb_i2c_set_bsc_reg_defaults`.
    fn set_bsc_reg_defaults(&mut self, hw: &mut impl Hw) {
        if self.data_regsz == 4 {
            self.ctlhi_reg = BSC_CTLHI_REG_DATAREG_SIZE_MASK;
        } else {
            self.ctlhi_reg &= !BSC_CTLHI_REG_DATAREG_SIZE_MASK;
        }
        hw.write(self.block, CTLHI_REG, self.ctlhi_reg);
        self.set_bus_speed(hw);
    }
}

/// The two DDC controllers and the `bsc_irq` controller they share.
pub struct Bsc {
    pub l2: L2Intc,
    pub dev: [Option<BrcmstbI2cDev>; 2],
}

impl Bsc {
    pub fn new(l2: L2Intc) -> Self {
        Bsc { l2, dev: [None, None] }
    }

    /// `brcmstb_i2c_probe` of DDC `n` on `bsc_irq` line `irq`, at
    /// `clock-frequency` `clk_freq_hz`, 4-byte data registers
    /// (`brcm,brcmstb-i2c`).
    pub fn probe(&mut self, hw: &mut impl Hw, n: usize, irq: u32, clk_freq_hz: u32) {
        let mut dev = BrcmstbI2cDev {
            block: Block::Ddc(n as u8),
            irq,
            ctl_reg: 0,
            iic_enable: 0,
            ctlhi_reg: 0,
            done: false,
            clk_freq_hz,
            data_regsz: 4,
        };
        dev.enable_disable_irq(hw, false);
        self.l2.request_irq(hw, irq);
        dev.set_bsc_reg_defaults(hw);
        self.dev[n] = Some(dev);
    }

    fn d(&mut self, n: usize) -> &mut BrcmstbI2cDev {
        self.dev[n].as_mut().expect("probed")
    }

    /// `brcmstb_l2_intc_irq_handle` of `bsc_irq` with the level flow of
    /// each pending line; a line no controller requested is left to the
    /// controller's mask.
    pub fn handle_irq(&mut self, hw: &mut impl Hw) {
        let mut status = self.l2.pending(hw);
        while status != 0 {
            let line = status.trailing_zeros();
            status &= !(1 << line);
            if !self.l2.level_enter(hw, line) { continue; }
            let mut ret = IrqReturn::None;
            for d in self.dev.iter_mut().flatten() {
                if d.irq == line {
                    ret = d.isr(hw);
                }
            }
            self.l2.level_exit(hw, line, ret);
        }
    }

    /// `brcmstb_i2c_wait_for_completion`, interrupt mode: wait for the GIC
    /// line, run the chained handler, until the ISR completed the transfer.
    fn wait_for_completion(&mut self, hw: &mut impl Hw, n: usize, mut since: u64) -> Result<(), i32> {
        let deadline = hw.now_us() + I2C_TIMEOUT as u64 * 1000;
        let mut ret = Ok(());
        while !self.d(n).done {
            let now = hw.now_us();
            if now >= deadline || !hw.irq_wait(Spi::BscIrq, since, ((deadline - now) / 1000).max(1) as u32) {
                ret = Err(-ETIMEDOUT);
                break;
            }
            since = hw.irq_arm(Spi::BscIrq);
            self.handle_irq(hw);
        }
        if ret.is_err() {
            self.d(n).enable_disable_irq(hw, false);
        }
        ret
    }

    /// `brcmstb_send_i2c_cmd`.
    fn send_i2c_cmd(&mut self, hw: &mut impl Hw, n: usize, cmd: BscXferCmd) -> Result<(), i32> {
        self.d(n).wait_if_busy(hw)?;
        self.d(n).done = false;
        let since = hw.irq_arm(Spi::BscIrq);
        let d = self.d(n);
        d.enable_disable_irq(hw, true);
        d.iic_enable |= BSC_IIC_EN_ENABLE_MASK;
        hw.write(d.block, IIC_ENABLE, d.iic_enable);
        let mut rc = self.wait_for_completion(hw, n, since);
        let d = self.d(n);
        if rc.is_ok()
            && (cmd == BscXferCmd::Rd || cmd == BscXferCmd::Wr)
            && hw.read(d.block, IIC_ENABLE) & BSC_IIC_EN_NOACK_MASK != 0
        {
            rc = Err(-EREMOTEIO);
        }
        hw.write(d.block, CNT_REG, 0);
        hw.write(d.block, IIC_ENABLE, 0);
        rc
    }

    /// `brcmstb_i2c_xfer_bsc_data`.
    fn xfer_bsc_data(&mut self, hw: &mut impl Hw, n: usize, buf: &mut [u8], flags: u16) -> Result<(), i32> {
        let len = buf.len() as u32;
        let d = self.d(n);
        let data_regsz = d.data_regsz;
        let no_ack = flags & I2C_M_IGNORE_NAK != 0;
        let rd = flags & I2C_M_RD != 0;
        let cmd = match (no_ack, rd) {
            (true, true) => BscXferCmd::RdNoack,
            (true, false) => BscXferCmd::WrNoack,
            (false, true) => BscXferCmd::Rd,
            (false, false) => BscXferCmd::Wr,
        };
        if no_ack {
            d.ctlhi_reg |= BSC_CTLHI_REG_IGNORE_ACK_MASK;
        } else {
            d.ctlhi_reg &= !BSC_CTLHI_REG_IGNORE_ACK_MASK;
        }
        hw.write(d.block, CTLHI_REG, d.ctlhi_reg);
        let ctl_reg = d.ctl_reg & !BSC_CTL_REG_DTF_MASK;
        d.ctl_reg = if matches!(cmd, BscXferCmd::Wr | BscXferCmd::WrNoack) {
            ctl_reg | DTF_WR_MASK
        } else {
            ctl_reg | DTF_RD_MASK
        };
        hw.write(d.block, CNT_REG, bsc_cnt_reg1_mask(data_regsz) & (len << BSC_CNT_REG1_SHIFT));
        if matches!(cmd, BscXferCmd::Wr | BscXferCmd::WrNoack) {
            let (mut cnt, mut i) = (0, 0);
            while cnt < len {
                let mut word: u32 = 0;
                for byte in 0..data_regsz {
                    word >>= 8;
                    if cnt + byte < len {
                        word |= (buf[(cnt + byte) as usize] as u32) << (8 * (data_regsz - 1));
                    }
                }
                hw.write(d.block, DATA_IN + 4 * i, word);
                cnt += data_regsz;
                i += 1;
            }
        }
        self.send_i2c_cmd(hw, n, cmd)?;
        if matches!(cmd, BscXferCmd::Rd | BscXferCmd::RdNoack) {
            let block = self.d(n).block;
            let (mut cnt, mut i) = (0, 0);
            while cnt < len {
                let mut data = hw.read(block, DATA_OUT + 4 * i);
                let mut byte = 0;
                while byte < data_regsz && byte + cnt < len {
                    buf[(cnt + byte) as usize] = data as u8;
                    data >>= 8;
                    byte += 1;
                }
                cnt += data_regsz;
                i += 1;
            }
        }
        Ok(())
    }

    /// `brcmstb_i2c_write_data_byte`.
    fn write_data_byte(&mut self, hw: &mut impl Hw, n: usize, b: u8, nak_expected: bool) -> Result<(), i32> {
        let cmd = if nak_expected { BscXferCmd::Wr } else { BscXferCmd::WrNoack };
        let block = self.d(n).block;
        hw.write(block, CNT_REG, 1);
        hw.write(block, DATA_IN, b as u32);
        self.send_i2c_cmd(hw, n, cmd)
    }

    /// `brcmstb_i2c_do_addr`.
    fn do_addr(&mut self, hw: &mut impl Hw, n: usize, msg: &I2cMsg) -> Result<(), i32> {
        let block = self.d(n).block;
        if msg.flags & I2C_M_TEN != 0 {
            // `i2c_10bit_addr_hi_from_msg`, `i2c_10bit_addr_lo_from_msg`.
            let hi = 0xf0 | ((msg.addr >> 7) as u8 & 0x06);
            hw.write(block, CHIP_ADDRESS, (hi & !(I2C_M_RD as u8)) as u32);
            if self.write_data_byte(hw, n, msg.addr as u8, false).is_err() {
                return Err(-EREMOTEIO);
            }
            if msg.flags & I2C_M_RD != 0 {
                self.d(n).set_i2c_start_stop(COND_RESTART | COND_NOSTOP);
                if self.write_data_byte(hw, n, hi | I2C_M_RD as u8, false).is_err() {
                    return Err(-EREMOTEIO);
                }
            }
        } else {
            // `i2c_8bit_addr_from_msg`.
            let addr = ((msg.addr << 1) | (msg.flags & I2C_M_RD)) as u8 as u32;
            hw.write(block, CHIP_ADDRESS, addr);
        }
        Ok(())
    }

    /// `brcmstb_i2c_xfer` (through `__i2c_transfer`, whose single try the
    /// adapter's zero `retries` leaves): the number of messages, or an
    /// errno.
    pub fn xfer(&mut self, hw: &mut impl Hw, n: usize, msgs: &mut [I2cMsg]) -> Result<usize, i32> {
        let num = msgs.len();
        let xfersz = self.d(n).get_xfersz();
        for i in 0..num {
            let cond = if i + 1 < num && msgs[i + 1].flags & I2C_M_NOSTART != 0 {
                !COND_START_STOP
            } else {
                COND_RESTART | COND_NOSTOP
            };
            self.d(n).set_i2c_start_stop(cond);
            if msgs[i].flags & I2C_M_NOSTART == 0 {
                self.do_addr(hw, n, &msgs[i])?;
            }
            let flags = msgs[i].flags;
            let mut cond_per_msg = cond;
            let mut off = 0;
            let mut len = msgs[i].buf.len() as u32;
            while len != 0 {
                let bytes_to_xfer = len.min(xfersz);
                if len <= xfersz {
                    cond_per_msg = if i == num - 1 {
                        cond_per_msg & !(COND_RESTART | COND_NOSTOP)
                    } else {
                        cond
                    };
                } else {
                    cond_per_msg = (cond_per_msg & !COND_RESTART) | COND_NOSTOP;
                }
                self.d(n).set_i2c_start_stop(cond_per_msg);
                let end = off + bytes_to_xfer as usize;
                self.xfer_bsc_data(hw, n, &mut msgs[i].buf[off..end], flags)?;
                len -= bytes_to_xfer;
                off = end;
                cond_per_msg = COND_NOSTART | COND_NOSTOP;
            }
        }
        Ok(num)
    }
}

/// One DDC controller as an `i2c_adapter`.
pub struct Ddc<'a> {
    pub bsc: &'a mut Bsc,
    pub n: usize,
}

impl Ddc<'_> {
    /// `i2c_transfer`.
    pub fn transfer(&mut self, hw: &mut impl Hw, msgs: &mut [I2cMsg]) -> Result<usize, i32> {
        self.bsc.xfer(hw, self.n, msgs)
    }
}
