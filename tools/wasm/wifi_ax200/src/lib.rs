//! wifi_ax200 — Intel Wi-Fi 6 AX200 driver (WASM module)
//!
//! iwlwifi-mvm, device family 22000 (gen2). Strict 1:1 port of Linux 6.18.26.
//! Plan: WIFI_AX200.md. Uses the nopeekOS WASM Driver ABI (npk_pci_*,
//! npk_mmio_*, npk_dma_*) — the same ABI proven by the RTL8852BE `wifi` driver.
//!
//! Stage 0a: bind PCI, map BAR0, read HW_REV + HW_RF_ID to confirm the chip.
//! Stage 0b (this file): reset + APM bring-up to MAC-clock-ready, following
//! `_iwl_trans_pcie_start_hw` for family 22000 (non-integrated AX200):
//!   prepare_card_hw → clear_persistence_bit → sw_reset → apm_init(→activate_nic).
//! All register pokes are 1:1 with the Linux source (no guessed values).

#![no_std]

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

mod host;
mod regs;
use regs::*;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// AX200 transport state. Mirrors the bits of `struct iwl_trans_pcie` we use.
struct Ax200 {
    mmio: i32,
    ltr_enabled: bool,
}

impl Ax200 {
    // ── CSR direct register access (BAR0) ────────────────────────
    fn r32(&self, reg: u32) -> u32 { host::mmio_r32(self.mmio, reg) }
    fn w32(&self, reg: u32, val: u32) { host::mmio_w32(self.mmio, reg, val); }
    /// iwl_set_bit: RMW preserving existing bits.
    fn set_bit(&self, reg: u32, bits: u32) { host::mmio_set32(self.mmio, reg, bits); }

    // ── PRPH access through HBUS (iwl_trans_pcie_read/write_prph) ─
    // umac_prph_offset is 0 for AX200, so umac == regular prph.
    fn prph_read(&self, reg: u32) -> u32 {
        self.w32(HBUS_TARG_PRPH_RADDR, (reg & PRPH_MASK) | (3 << 24));
        self.r32(HBUS_TARG_PRPH_RDAT)
    }
    fn prph_write(&self, reg: u32, val: u32) {
        self.w32(HBUS_TARG_PRPH_WADDR, (reg & PRPH_MASK) | (3 << 24));
        self.w32(HBUS_TARG_PRPH_WDAT, val);
    }

    /// iwl_poll_bit: spin until (reg & mask) == mask or timeout (microseconds).
    /// IWL_POLL_INTERVAL is 10us in Linux; we lack a us timer, so we pace with
    /// a short busy-spin (small timeouts) or a yielding 1ms sleep (large ones).
    fn poll_bit(&self, reg: u32, mask: u32, timeout_us: u32) -> bool {
        let mut waited = 0u32;
        loop {
            if self.r32(reg) & mask == mask { return true; }
            if waited >= timeout_us { return false; }
            if timeout_us >= 1000 {
                host::sleep_ms(1);
                waited += 1000;
            } else {
                for _ in 0..64 { core::hint::spin_loop(); }
                waited += 10;
            }
        }
    }

    // ── iwl_pcie_set_hw_ready / prepare_card_hw (trans.c) ────────
    fn set_hw_ready(&self) -> bool {
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_PCI_OWN_SET);
        let ready = self.poll_bit(
            CSR_HW_IF_CONFIG_REG,
            CSR_HW_IF_CONFIG_REG_PCI_OWN_SET,
            HW_READY_TIMEOUT_US,
        );
        if ready {
            self.set_bit(CSR_MBOX_SET_REG, CSR_MBOX_SET_REG_OS_ALIVE);
        }
        ready
    }

    /// Returns true once the card is ready (owned by us, not AMT/ME).
    fn prepare_card_hw(&self) -> bool {
        if self.set_hw_ready() {
            return true;
        }
        self.set_bit(CSR_DBG_LINK_PWR_MGMT_REG, CSR_RESET_LINK_PWR_MGMT_DISABLED);
        host::sleep_ms(2); // usleep_range(1000, 2000)

        for _ in 0..10 {
            // Prepare conditions to check again: wake the management bus.
            self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_WAKE_ME);
            // Inner do-while: up to 150ms (Linux uses 200us steps; we use 1ms).
            let mut t = 0u32;
            loop {
                if self.set_hw_ready() {
                    return true;
                }
                // No iwl_mei (CSME) in our world — that branch is skipped.
                host::sleep_ms(1);
                t += 1000;
                if t >= 150_000 {
                    break;
                }
            }
            host::sleep_ms(25);
        }
        false
    }

    // ── iwl_trans_pcie_sw_reset (trans.c) ───────────────────────
    // AX200 family 22000 < BZ → CSR_RESET path, usleep_range(5000, 6000).
    fn sw_reset(&self, retake_ownership: bool) -> bool {
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        host::sleep_ms(6);
        if retake_ownership {
            self.prepare_card_hw()
        } else {
            true
        }
    }

    // ── iwl_trans_pcie_clear_persistence_bit (trans.c) ──────────
    // Family 22000 → wprot = PREG_PRPH_WPROT_22000. Returns false only on
    // the unrecoverable -EPERM path.
    fn clear_persistence_bit(&self) -> bool {
        let hpm = self.prph_read(HPM_DEBUG);
        if !is_hw_error_value(hpm) && (hpm & PERSISTENCE_BIT) != 0 {
            let wprot_val = self.prph_read(PREG_PRPH_WPROT_22000);
            if wprot_val & PREG_WFPM_ACCESS != 0 {
                host::print("[ax200] error: can not clear persistence bit\n");
                return false;
            }
            self.prph_write(HPM_DEBUG, hpm & !PERSISTENCE_BIT);
        }
        true
    }

    // ── iwl_pcie_apm_config (trans.c) ───────────────────────────
    // L0s is unstable on these devices: always set L0S_DISABLED. Then cache
    // ASPM / LTR capability for later (set_pwr / set_ltr in Stage 2).
    fn apm_config(&mut self) {
        self.set_bit(CSR_GIO_REG, CSR_GIO_REG_VAL_L0S_DISABLED);

        let cap = pcie_find_cap(PCI_CAP_ID_EXP);
        if cap != 0 {
            let _lctl = pci_read16(cap + PCI_EXP_LNKCTL);
            // pm_support = !(lctl & ASPM_L0S) — not needed until power mgmt.
            let cap2 = pci_read16(cap + PCI_EXP_DEVCTL2);
            self.ltr_enabled = cap2 & PCI_EXP_DEVCTL2_LTR_EN != 0;
        }
        host::print("[ax200] apm_config: LTR ");
        host::print(if self.ltr_enabled { "enabled\n" } else { "disabled\n" });
    }

    // ── iwl_pcie_gen1_2_activate_nic (trans.c) ──────────────────
    // bisr_workaround = 1 for AX200: mdelay(2) before, udelay(200) after.
    // Family < BZ → INIT_DONE, poll MAC_CLOCK_READY.
    fn activate_nic(&self) -> bool {
        host::sleep_ms(2); // bisr_workaround: TOP FSM settle

        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_INIT_DONE);

        let ok = self.poll_bit(
            CSR_GP_CNTRL,
            CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY,
            MAC_CLOCK_TIMEOUT_US,
        );
        if !ok {
            host::print("[ax200] failed to wake NIC (MAC clock not ready)\n");
        }

        for _ in 0..1024 { core::hint::spin_loop(); } // bisr_workaround: udelay(200)
        ok
    }

    // ── iwl_pcie_apm_init (trans.c, gen1 — the start_hw path) ────
    // Family 22000: DIS_L0S_EXIT_TIMER (family < 8000) skipped; no pll_cfg;
    // no host_interrupt_operation_mode.
    fn apm_init(&mut self) -> bool {
        // Disable L0s without affecting L1 (ICH bug W/A).
        self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);
        // FH wait threshold to maximum (HW error during stress W/A).
        self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);
        // HAP INTA: wake the PCIe link L1a -> L0s.
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_HAP_WAKE);

        self.apm_config();
        // pll_cfg: AX200 base has none → no CSR_ANA_PLL_CFG.
        self.activate_nic()
    }

    // ── _iwl_trans_pcie_start_hw (trans.c) ──────────────────────
    fn start_hw(&mut self) -> bool {
        if !self.prepare_card_hw() {
            host::print("[ax200] error while preparing HW (AMT owns the device?)\n");
            return false;
        }
        host::print("[ax200] card prepared (ownership taken)\n");

        if !self.clear_persistence_bit() {
            return false;
        }

        if !self.sw_reset(true) {
            host::print("[ax200] sw_reset: card not ready after reset\n");
            return false;
        }
        host::print("[ax200] sw_reset done\n");

        // force_power_gating: family==22000 && integrated. AX200 is a discrete
        // M.2 card (not integrated) → skipped.

        if !self.apm_init() {
            return false;
        }
        host::print("[ax200] apm_init done — MAC clock ready\n");
        true
    }
}

/// iwl_trans_is_hw_error_value (iwl-trans.h).
fn is_hw_error_value(val: u32) -> bool {
    (val & !0xf) == 0xa5a5_a5a0 || (val & !0xf) == 0x5a5a_5a50
}

/// Read a 16-bit value from PCI config space (dword read + extract).
fn pci_read16(off: u8) -> u16 {
    let dword = host::pci_read_config(off & !0x3);
    ((dword >> ((off & 0x3) * 8)) & 0xFFFF) as u16
}

/// Walk the PCI capability list for the given capability ID. Returns the
/// config-space offset of the capability, or 0 if absent.
fn pcie_find_cap(id: u8) -> u8 {
    let mut ptr = (host::pci_read_config(PCI_CAP_PTR as u8) & 0xFC) as u8;
    let mut guard = 0;
    while ptr != 0 && guard < 48 {
        let hdr = host::pci_read_config(ptr);
        if (hdr & 0xFF) as u8 == id {
            return ptr;
        }
        ptr = ((hdr >> 8) & 0xFC) as u8;
        guard += 1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[ax200] Intel Wi-Fi 6 AX200 driver v0.2.1 — Stage 0b (reset + APM)\n");

    // ── Stage 0a: bind, bus master, map BAR0, identity ───────────
    let rc = host::pci_bind(AX200_VENDOR, AX200_DEVICE);
    if rc != 0 {
        host::print("[ax200] PCI bind failed (");
        match rc {
            -1 => host::print("not found"),
            -2 => host::print("denied"),
            _ => host::print("unknown error"),
        }
        host::print(")\n");
        return;
    }
    host::print("[ax200] PCI bind OK\n");

    host::pci_enable_bus_master();

    let mmio = host::mmio_map_bar(BAR_CSR, 16);
    if mmio < 0 {
        host::print("[ax200] BAR0 map failed\n");
        return;
    }
    host::print("[ax200] BAR0 mapped\n");

    let mut dev = Ax200 { mmio, ltr_enabled: false };

    let hw_rev = dev.r32(CSR_HW_REV);
    let rf_id = dev.r32(CSR_HW_RF_ID);
    host::log_reg("CSR_HW_REV", hw_rev);
    host::log_reg("CSR_HW_RF_ID", rf_id);

    if hw_rev == 0xFFFF_FFFF || hw_rev == 0 {
        host::print("[ax200] HW_REV looks wrong — chip not responding?\n");
        return;
    }
    // RF type lives in the high nibble pattern; HR is the AX200's radio.
    if rf_id & 0x0FFF_F000 == CSR_HW_RF_ID_TYPE_HR & 0x0FFF_F000 {
        host::print("[ax200] RF type = HR (matches AX200) — Stage 0a OK\n");
    } else {
        host::print("[ax200] RF type unexpected (continuing anyway)\n");
    }

    // ── Stage 0b: reset + APM bring-up ───────────────────────────
    if dev.start_hw() {
        host::print("[ax200] Stage 0b OK — chip powered up, PRPH accessible\n");
    } else {
        host::print("[ax200] Stage 0b FAILED\n");
    }

    // Probe-stage driver: return so the fiber ends and the core is freed.
    // npk_input_wait HLTs without yielding to the scheduler, so idling here
    // would pin the core and starve the shell. A persistent run-loop comes
    // with the RX/event path (Stage 4+) and must use a yielding primitive.
}
