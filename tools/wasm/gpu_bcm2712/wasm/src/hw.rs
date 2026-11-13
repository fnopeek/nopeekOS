//! `gpu_bcm2712_core::hw::Hw` through the kernel: the display set's
//! windows and interrupts (`npk_mmio_map_dt`, `npk_irq_register_dt`, in
//! the part order of `kernel/src/platform/pi5_display_set.rs`), the
//! firmware clocks and the timer.

use gpu_bcm2712_core::hw::{Block, ClockOp, HdmiReg, Hw, Spi};

/// The display-set parts.
const HVS: usize = 0;
const PV0: usize = 1;
const DISP_INTR: usize = 3;
const DVP: usize = 4;
const HDMI0: usize = 5;
const DDC0: usize = 7;
const BSC_IRQ: usize = 9;
const AON_INTR: usize = 10;
/// V3D: windows 0 hub below its MMU, 1 hub above it, 2 core0; interrupts
/// 0 hub, 1 core.
const V3D: usize = 11;
const PARTS: usize = 12;
/// Where the kernel's window gap over V3D's MMU registers lies in the hub.
const V3D_HUB_GAP: u32 = 0x1000;
const V3D_HUB_HI: u32 = 0x1300;

/// An HDMI node's windows, in its `reg-names` order (`HdmiReg`).
const HDMI_REGS: usize = 9;

/// The parts with a GIC line of their own.
const IRQ_PARTS: [usize; 5] = [PV0, PV0 + 1, DISP_INTR, BSC_IRQ, AON_INTR];

pub struct PiHw {
    /// `[part][reg]` → handle.
    maps: [[i32; HDMI_REGS]; PARTS],
    /// Vector of each of `IRQ_PARTS`.
    vectors: [i32; IRQ_PARTS.len()],
    /// V3D's hub and core interrupt vectors, once `map_v3d` ran.
    v3d_vectors: [i32; 2],
}

/// The lines that fired since they were last armed.
pub struct Fired {
    pub pv: [bool; 2],
    pub disp_intr: bool,
    pub aon_intr: bool,
}

impl PiHw {
    /// Every window and interrupt of the bound display set.
    pub fn map() -> Option<Self> {
        let mut maps = [[-1; HDMI_REGS]; PARTS];
        for (part, m) in maps.iter_mut().enumerate().take(V3D) {
            let regs = if part == HDMI0 || part == HDMI0 + 1 { HDMI_REGS } else { 1 };
            for (reg, h) in m.iter_mut().enumerate().take(regs) {
                *h = npk_sys::mmio_map_dt(part as i32, reg as i32);
                if *h < 0 {
                    return None;
                }
            }
        }
        let mut vectors = [-1; IRQ_PARTS.len()];
        for (v, &part) in vectors.iter_mut().zip(IRQ_PARTS.iter()) {
            *v = npk_sys::irq_register_dt(part as i32, 0);
            if *v < 0 {
                return None;
            }
        }
        Some(PiHw { maps, vectors, v3d_vectors: [-1; 2] })
    }

    /// V3D's windows and interrupts, after `npk_v3d_open`.
    pub fn map_v3d(&mut self) -> bool {
        for reg in 0..3 {
            let h = npk_sys::mmio_map_dt(V3D as i32, reg as i32);
            if h < 0 {
                return false;
            }
            self.maps[V3D][reg] = h;
        }
        for (i, v) in self.v3d_vectors.iter_mut().enumerate() {
            *v = npk_sys::irq_register_dt(V3D as i32, i as i32);
            if *v < 0 {
                return false;
            }
        }
        true
    }

    fn handle(&self, b: Block) -> i32 {
        match b {
            Block::Hvs => self.maps[HVS][0],
            Block::Pv(n) => self.maps[PV0 + n as usize][0],
            Block::DispIntr => self.maps[DISP_INTR][0],
            Block::Dvp => self.maps[DVP][0],
            Block::Hdmi(n, r) => self.maps[HDMI0 + n as usize][r as usize],
            Block::Ddc(n) => self.maps[DDC0 + n as usize][0],
            Block::BscIrq => self.maps[BSC_IRQ][0],
            Block::AonIntr => self.maps[AON_INTR][0],
            Block::V3dHub => self.maps[V3D][0],
            Block::V3dCore => self.maps[V3D][2],
        }
    }

    /// The window and offset of `off` in block `b`: V3D's hub continues in
    /// its second window above the MMU's registers.
    fn at(&self, b: Block, off: u32) -> (i32, u32) {
        match b {
            Block::V3dHub if off >= V3D_HUB_HI => (self.maps[V3D][1], off - V3D_HUB_HI),
            Block::V3dHub if off >= V3D_HUB_GAP => (-1, off),
            _ => (self.handle(b), off),
        }
    }

    fn vector(&self, s: Spi) -> i32 {
        match s {
            Spi::V3dHub => return self.v3d_vectors[0],
            Spi::V3dCore => return self.v3d_vectors[1],
            _ => {}
        }
        let part = match s {
            Spi::Pv(n) => PV0 + n as usize,
            Spi::DispIntr => DISP_INTR,
            Spi::BscIrq => BSC_IRQ,
            Spi::AonIntr => AON_INTR,
            Spi::V3dHub | Spi::V3dCore => unreachable!("V3D's lines are its own"),
        };
        let i = IRQ_PARTS.iter().position(|&p| p == part).expect("a part with a GIC line");
        self.vectors[i]
    }

    /// The fired counts of the lines the main loop serves, each line
    /// unmasked again.
    pub fn arm_all(&mut self) -> [i64; 4] {
        [Spi::Pv(0), Spi::Pv(1), Spi::DispIntr, Spi::AonIntr].map(|s| npk_sys::irq_arm(self.vector(s)))
    }

    /// Which of the main loop's lines fired since `seen`.
    pub fn fired(&mut self, seen: &mut [i64; 4]) -> Fired {
        let now = self.arm_all();
        let f = Fired {
            pv: [now[0] != seen[0], now[1] != seen[1]],
            disp_intr: now[2] != seen[2],
            aon_intr: now[3] != seen[3],
        };
        *seen = now;
        f
    }
}

impl Hw for PiHw {
    fn read(&mut self, b: Block, off: u32) -> u32 {
        let (h, off) = self.at(b, off);
        npk_sys::mmio_read32(h, off as i32) as u32
    }

    fn write(&mut self, b: Block, off: u32, v: u32) {
        let (h, off) = self.at(b, off);
        npk_sys::mmio_write32(h, off as i32, v as i32);
    }

    fn now_us(&mut self) -> u64 {
        npk_sys::now_us() as u64
    }

    /// `usleep_range` / `msleep`: whole milliseconds sleep, the rest spins.
    fn delay_us(&mut self, us: u64) {
        let end = self.now_us() + us;
        if us >= 1000 {
            npk_sys::sleep((us / 1000) as i32);
        }
        while self.now_us() < end {}
    }

    fn irq_arm(&mut self, s: Spi) -> u64 {
        npk_sys::irq_arm(self.vector(s)) as u64
    }

    fn irq_wait(&mut self, s: Spi, since: u64, timeout_ms: u32) -> bool {
        npk_sys::irq_wait(self.vector(s), since as i64, timeout_ms as i32) == 1
    }

    fn fw_clock(&mut self, id: u32, op: ClockOp) -> Option<u32> {
        let (op, rate) = match op {
            ClockOp::Rate => (0, 0),
            ClockOp::MaxRate => (1, 0),
            ClockOp::MinRate => (2, 0),
            ClockOp::SetRate(r) => (3, r as i64),
            ClockOp::Enable => (4, 0),
            ClockOp::Disable => (5, 0),
        };
        let r = npk_sys::fw_clock(id as i32, op, rate);
        (r >= 0).then_some(r as u32)
    }

    fn fw_display_done(&mut self) -> bool {
        npk_sys::fw_display_done() == 0
    }
}

/// `HdmiReg` is the index into an HDMI node's windows.
const _: () = assert!(HdmiReg::Hd as usize == HDMI_REGS - 1);
