//! The Linux kernel services amdgpu calls, on top of the nopeekOS driver ABI.
//!
//! The module is single-threaded: every amdgpu spinlock and mutex guards
//! nothing here and is left out. Timers (`hrtimer`) are deadlines checked
//! by the driver loop and by every wait, so they fire no earlier than in
//! Linux and at most one poll later.

/// Microseconds since boot.
pub fn now_us() -> u64 {
    let t = npk_sys::now_us();
    if t < 0 { 0 } else { t as u64 }
}

/// `udelay`: spins on the clock.
pub fn udelay(us: u64) {
    let t0 = now_us();
    while now_us() - t0 < us {}
}

/// `wait_for_atomic(COND, ms)` / `_wait_for_atomic(COND, us, 0)`: busy-polls
/// `cond` until it holds or `timeout_us` passes. `Ok` when it held. The
/// condition is checked once more after the deadline, as Linux does.
pub fn wait_for_atomic_us(timeout_us: u64, mut cond: impl FnMut() -> bool) -> Result<(), ()> {
    let t0 = now_us();
    loop {
        let expired = now_us() - t0 > timeout_us;
        if cond() {
            return Ok(());
        }
        if expired {
            return Err(());
        }
    }
}

/// `wait_for(COND, ms)`: polls with sleeps that grow from 10 us to 1 ms.
pub fn wait_for_ms(timeout_ms: u64, cond: impl FnMut() -> bool) -> Result<(), ()> {
    _wait_for(timeout_ms * 1000, 10, 1000, cond)
}

/// `_wait_for(COND, US, Wmin, Wmax)`: polls with sleeps that double from
/// `wmin` up to `wmax` microseconds.
pub fn _wait_for(timeout_us: u64, wmin: u64, wmax: u64, mut cond: impl FnMut() -> bool) -> Result<(), ()> {
    let t0 = now_us();
    let mut wait_us = wmin;
    loop {
        let expired = now_us() - t0 > timeout_us;
        if cond() {
            return Ok(());
        }
        if expired {
            return Err(());
        }
        if wait_us >= 1000 {
            npk_sys::sleep((wait_us / 1000) as i32);
        } else {
            udelay(wait_us);
        }
        if wait_us < wmax {
            wait_us <<= 1;
        }
    }
}

/// A `struct hrtimer` reduced to its deadline.
#[derive(Clone, Copy, Default)]
pub struct HrTimer {
    pub expires_us: Option<u64>,
}

impl HrTimer {
    /// `hrtimer_start_range_ns(timer, ns, ...)`, relative.
    pub fn start(&mut self, delay_us: u64) {
        self.expires_us = Some(now_us() + delay_us);
    }

    /// `hrtimer_cancel`: true when the timer was queued.
    pub fn cancel(&mut self) -> bool {
        self.expires_us.take().is_some()
    }

    /// `hrtimer_active`.
    pub fn active(&self) -> bool {
        self.expires_us.is_some()
    }

    /// Due now: the deadline has passed.
    pub fn due(&self, now: u64) -> bool {
        self.expires_us.is_some_and(|t| now >= t)
    }
}

/// One log line, built whole so the kernel log keeps it on one line.
pub struct Line {
    buf: [u8; 192],
    len: usize,
}

impl Line {
    pub fn new() -> Self {
        let mut l = Line { buf: [0; 192], len: 0 };
        l.s("[gpu_amd_renoir] ");
        l
    }

    pub fn s(&mut self, s: &str) -> &mut Self {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        self
    }

    /// Raw bytes, as `%s` prints them.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        let n = b.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
        self
    }

    pub fn hex(&mut self, v: u32) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut b = *b"0x00000000";
        for i in 0..8 {
            b[9 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
        }
        self.s(core::str::from_utf8(&b).unwrap_or("?"))
    }

    /// `%0<n>X`: `n` upper-case hex digits.
    pub fn hexw(&mut self, v: u32, n: usize) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut b = [b'0'; 8];
        let n = n.min(8);
        for i in 0..n {
            b[n - 1 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
        }
        self.s(core::str::from_utf8(&b[..n]).unwrap_or("?"))
    }

    pub fn dec(&mut self, mut v: u64) -> &mut Self {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(core::str::from_utf8(&b[i..]).unwrap_or("?"))
    }

    /// To the kernel log: `dmesg`, the terminal's log stream and `debug`
    /// all show it, once.
    pub fn say(&self) {
        npk_sys::log(&self.buf[..self.len]);
    }

    /// `drm_dbg` / `drm_dbg_kms`: to the log only with `set log.drivers 1`.
    pub fn dbg(&self) {
        if debug_enabled() {
            self.say();
        }
    }
}

pub fn say(s: &str) {
    Line::new().s(s).say();
}

pub fn dbg(s: &str) {
    Line::new().s(s).dbg();
}

/// `set log.drivers 1`, asked at each debug line so it can be switched while
/// the module runs.
pub fn debug_enabled() -> bool {
    npk_sys::sys_info(50) == 1
}

/// `drm_err`, `drm_WARN` with a true condition, `drm_info`.
pub fn drm_err(s: &str) {
    Line::new().s("error: ").s(s).say();
}

/// `struct firmware`: the bytes of one firmware file.
pub struct firmware {
    pub data: alloc::vec::Vec<u8>,
}

/// `request_firmware(fw, name, dev)`: `name` as Linux names it
/// ("amdgpu/renoir_pfp.bin"), read from sys/firmware/, where `install`
/// puts the files the module's manifest entry lists. -ENOENT when absent.
pub fn request_firmware(name: &[u8]) -> Result<firmware, i32> {
    let mut path = alloc::vec::Vec::with_capacity(13 + name.len());
    path.extend_from_slice(b"sys/firmware/");
    path.extend_from_slice(name);
    let mut st = [0u8; 17];
    if npk_sys::fs_stat(&path, &mut st) <= 0 || st[8] != 0 {
        return Err(-2);
    }
    let size = u64::from_le_bytes(st[..8].try_into().unwrap_or([0; 8])) as usize;
    let mut data = alloc::vec![0u8; size];
    if size == 0 || npk_sys::fetch(&path, &mut data) != size as i32 {
        return Err(-2);
    }
    Ok(firmware { data })
}

/// `drm_info`.
pub fn drm_info(s: &str) {
    Line::new().s(s).say();
}

/// `struct pci_dev`, reduced to its bus address; the config space is read
/// as Linux reads it (`pci_read_config_dword` on dword offsets).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct pci_dev {
    pub bus: u8,
    pub devfn: u8,
}

impl pci_dev {
    pub fn new(bus: u8, slot: u8, func: u8) -> Self {
        pci_dev { bus, devfn: slot << 3 | func }
    }

    /// `pci_read_config_dword`.
    pub fn read_dword(&self, off: u32) -> u32 {
        npk_sys::pci_read_config_at((self.bus as i32) << 8 | self.devfn as i32, (off & !3) as i32) as u32
    }

    /// `pci_read_config_word`.
    pub fn read_word(&self, off: u32) -> u16 {
        (self.read_dword(off) >> ((off & 2) * 8)) as u16
    }

    /// `pci_read_config_byte`.
    pub fn read_byte(&self, off: u32) -> u8 {
        (self.read_dword(off) >> ((off & 3) * 8)) as u8
    }

    pub fn vendor(&self) -> u16 {
        self.read_word(0)
    }

    /// `pci_find_capability(dev, cap)`.
    pub fn find_capability(&self, cap: u8) -> u32 {
        use crate::regs::*;
        if self.read_word(PCI_STATUS) as u32 & PCI_STATUS_CAP_LIST == 0 {
            return 0;
        }
        let mut pos = self.read_byte(PCI_CAPABILITY_LIST) as u32 & !3;
        // PCI_FIND_CAP_TTL
        for _ in 0..48 {
            if pos < 0x40 {
                break;
            }
            let id = self.read_byte(pos);
            if id == 0xff {
                break;
            }
            if id == cap {
                return pos;
            }
            pos = self.read_byte(pos + 1) as u32 & !3;
        }
        0
    }

    /// `pci_pcie_cap`.
    pub fn pcie_cap(&self) -> u32 {
        self.find_capability(crate::regs::PCI_CAP_ID_EXP as u8)
    }

    /// `pcie_caps_reg`.
    fn pcie_caps_reg(&self) -> u32 {
        let c = self.pcie_cap();
        if c == 0 { 0 } else { self.read_word(c + crate::regs::PCI_EXP_FLAGS) as u32 }
    }

    /// `pci_pcie_type`.
    pub fn pcie_type(&self) -> u32 {
        (self.pcie_caps_reg() & crate::regs::PCI_EXP_FLAGS_TYPE) >> 4
    }

    /// `pcie_cap_version`.
    fn pcie_cap_version(&self) -> u32 {
        self.pcie_caps_reg() & crate::regs::PCI_EXP_FLAGS_VERS
    }

    /// `pcie_cap_has_lnkctl`.
    fn pcie_cap_has_lnkctl(&self) -> bool {
        use crate::regs::*;
        matches!(self.pcie_type(), t if t == PCI_EXP_TYPE_ENDPOINT || t == PCI_EXP_TYPE_LEG_END
            || t == PCI_EXP_TYPE_ROOT_PORT || t == PCI_EXP_TYPE_UPSTREAM || t == PCI_EXP_TYPE_DOWNSTREAM
            || t == PCI_EXP_TYPE_PCI_BRIDGE || t == PCI_EXP_TYPE_PCIE_BRIDGE)
    }

    /// `pcie_capability_read_dword`, for the link registers.
    pub fn pcie_capability_read_dword(&self, pos: u32) -> u32 {
        use crate::regs::*;
        let c = self.pcie_cap();
        if c == 0 {
            return 0;
        }
        let implemented = if pos == PCI_EXP_LNKCAP {
            self.pcie_cap_has_lnkctl()
        } else if pos == PCI_EXP_LNKCAP2 {
            self.pcie_cap_has_lnkctl() && self.pcie_cap_version() > 1
        } else {
            false
        };
        if implemented { self.read_dword(c + pos) } else { 0 }
    }

    /// `pcie_get_supported_speeds`, as probe.c stores it in
    /// `dev->supported_speeds`.
    pub fn supported_speeds(&self) -> u32 {
        use crate::regs::*;
        let lnkcap2 = self.pcie_capability_read_dword(PCI_EXP_LNKCAP2);
        let mut speeds = lnkcap2 & PCI_EXP_LNKCAP2_SLS;
        // Ignore speeds higher than Max Link Speed
        let lnkcap = self.pcie_capability_read_dword(PCI_EXP_LNKCAP);
        let sls = lnkcap & PCI_EXP_LNKCAP_SLS;
        speeds &= if sls >= 31 { u32::MAX } else { (1u32 << (sls + 1)) - 1 };
        // PCIe r3.0-compliant
        if speeds != 0 {
            return speeds & 0xff;
        }
        // Synthesize from the Max Link Speed field
        if sls == PCI_EXP_LNKCAP_SLS_5_0GB {
            PCI_EXP_LNKCAP2_SLS_5_0GB | PCI_EXP_LNKCAP2_SLS_2_5GB
        } else if sls == PCI_EXP_LNKCAP_SLS_2_5GB {
            PCI_EXP_LNKCAP2_SLS_2_5GB
        } else {
            0
        }
    }

    /// `pcie_get_speed_cap`: `PCIE_LNKCAP2_SLS2SPEED(supported_speeds)`.
    pub fn pcie_get_speed_cap(&self) -> u32 {
        use crate::regs::*;
        let s = self.supported_speeds();
        if s & PCI_EXP_LNKCAP2_SLS_64_0GB != 0 { PCIE_SPEED_64_0GT }
        else if s & PCI_EXP_LNKCAP2_SLS_32_0GB != 0 { PCIE_SPEED_32_0GT }
        else if s & PCI_EXP_LNKCAP2_SLS_16_0GB != 0 { PCIE_SPEED_16_0GT }
        else if s & PCI_EXP_LNKCAP2_SLS_8_0GB != 0 { PCIE_SPEED_8_0GT }
        else if s & PCI_EXP_LNKCAP2_SLS_5_0GB != 0 { PCIE_SPEED_5_0GT }
        else if s & PCI_EXP_LNKCAP2_SLS_2_5GB != 0 { PCIE_SPEED_2_5GT }
        else { PCI_SPEED_UNKNOWN }
    }

    /// `pcie_get_width_cap`.
    pub fn pcie_get_width_cap(&self) -> u32 {
        use crate::regs::*;
        let lnkcap = self.pcie_capability_read_dword(PCI_EXP_LNKCAP);
        if lnkcap != 0 {
            (lnkcap & PCI_EXP_LNKCAP_MLW) >> PCI_EXP_LNKCAP_MLW.trailing_zeros()
        } else {
            PCIE_LNK_WIDTH_UNKNOWN
        }
    }

    /// The bridge whose secondary bus is this device's bus: every slot's
    /// function 0, the others of a multi-function device.
    fn parent_bridge(&self) -> Option<pci_dev> {
        use crate::regs::*;
        for bus in 0..=255u32 {
            for slot in 0..32u32 {
                let f0 = pci_dev { bus: bus as u8, devfn: (slot << 3) as u8 };
                let id = f0.read_dword(0);
                if id == 0xffff_ffff || id == 0 {
                    continue;
                }
                let funcs = if f0.read_byte(PCI_HEADER_TYPE) & 0x80 != 0 { 8 } else { 1 };
                for func in 0..funcs {
                    let d = pci_dev { bus: bus as u8, devfn: (slot << 3 | func) as u8 };
                    let id = d.read_dword(0);
                    if id == 0xffff_ffff || id == 0 {
                        continue;
                    }
                    if d.read_byte(PCI_HEADER_TYPE) as u32 & PCI_HEADER_TYPE_MASK == PCI_HEADER_TYPE_BRIDGE
                        && d.read_byte(PCI_SECONDARY_BUS) == self.bus
                    {
                        return Some(d);
                    }
                }
            }
        }
        None
    }

    /// `pci_is_root_bus(dev->bus)`: no bridge leads to the bus.
    pub fn on_root_bus(&self) -> bool {
        self.parent_bridge().is_none()
    }

    /// `pci_upstream_bridge`.
    pub fn upstream_bridge(&self) -> Option<pci_dev> {
        self.parent_bridge()
    }
}
