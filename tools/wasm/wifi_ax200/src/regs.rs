//! AX200 register definitions — verified 1:1 against Linux 6.18.26
//! `drivers/net/wireless/intel/iwlwifi/iwl-csr.h` (CSR_BASE = 0x000).
//!
//! Only offsets verified against the header live here. Bit masks are added
//! per stage as each function is ported (strict 1:1, no guessed values —
//! see memory/feedback_linux_strict.md). BAR0 carries the CSR block.

// ── PCI identity ─────────────────────────────────────────────────
pub const AX200_VENDOR: u16 = 0x8086;
pub const AX200_DEVICE: u16 = 0x2723; // iwl_ax200_mac_cfg, RF = HR, family 22000

// BAR carrying the CSR/PRPH register block (iwlwifi: BAR0).
pub const BAR_CSR: u8 = 0;

// ── CSR registers (iwl-csr.h, CSR_BASE = 0x000) ──────────────────
pub const CSR_HW_IF_CONFIG_REG: u32 = 0x000; // hardware interface config
pub const CSR_INT: u32 = 0x008; // host interrupt status/ack
pub const CSR_INT_MASK: u32 = 0x00C; // host interrupt enable
pub const CSR_FH_INT_STATUS: u32 = 0x010; // busmaster int status/ack
pub const CSR_RESET: u32 = 0x020; // busmaster enable, NMI, etc.
pub const CSR_GP_CNTRL: u32 = 0x024;
pub const CSR_HW_REV: u32 = 0x028;
pub const CSR_FUNC_SCRATCH: u32 = 0x02C; // FW debug scratch
pub const CSR_GIO_REG: u32 = 0x03C;
pub const CSR_UCODE_DRV_GP1: u32 = 0x054;
pub const CSR_MBOX_SET_REG: u32 = 0x088;
pub const CSR_HW_RF_ID: u32 = 0x09C;
pub const CSR_MAC_SHADOW_REG_CTRL: u32 = 0x0A8;
pub const CSR_GIO_CHICKEN_BITS: u32 = 0x100;
pub const CSR_DBG_HPET_MEM_REG: u32 = 0x240;
pub const CSR_DBG_LINK_PWR_MGMT_REG: u32 = 0x250;

// ── CSR bit masks (iwl-csr.h, verified) ──────────────────────────
// HW_IF_CONFIG
pub const CSR_HW_IF_CONFIG_REG_HAP_WAKE: u32 = 0x0008_0000;
pub const CSR_HW_IF_CONFIG_REG_PCI_OWN_SET: u32 = 0x0040_0000;
pub const CSR_HW_IF_CONFIG_REG_WAKE_ME: u32 = 0x0800_0000;
// MBOX_SET
pub const CSR_MBOX_SET_REG_OS_ALIVE: u32 = 0x0000_0020; // BIT(5)
// RESET
pub const CSR_RESET_REG_FLAG_SW_RESET: u32 = 0x0000_0080;
pub const CSR_RESET_LINK_PWR_MGMT_DISABLED: u32 = 0x8000_0000;
// GP_CNTRL
pub const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 0x0000_0001;
pub const CSR_GP_CNTRL_REG_FLAG_INIT_DONE: u32 = 0x0000_0004;
// GIO_CHICKEN / GIO / DBG_HPET
pub const CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX: u32 = 0x0080_0000;
pub const CSR_GIO_REG_VAL_L0S_DISABLED: u32 = 0x0000_0002;
pub const CSR_DBG_HPET_MEM_REG_VAL: u32 = 0xFFFF_0000;
// HW_RF_ID type (masked compare) — AX200 carries HR
pub const CSR_HW_RF_ID_TYPE_HR: u32 = 0x0010_A000;

// ── PRPH access via HBUS (iwl-csr.h, HBUS_BASE = 0x400) ───────────
pub const HBUS_TARG_PRPH_WADDR: u32 = 0x444;
pub const HBUS_TARG_PRPH_RADDR: u32 = 0x448;
pub const HBUS_TARG_PRPH_WDAT: u32 = 0x44C;
pub const HBUS_TARG_PRPH_RDAT: u32 = 0x450;
// PRPH address mask for family < AX210 (iwl_trans_pcie_prph_msk)
pub const PRPH_MASK: u32 = 0x000F_FFFF;

// ── PRPH registers / bits (iwl-prph.h) ───────────────────────────
pub const HPM_DEBUG: u32 = 0x00A0_3440;
pub const PERSISTENCE_BIT: u32 = 0x0000_1000; // BIT(12)
pub const PREG_PRPH_WPROT_22000: u32 = 0x00A0_4D00;
pub const PREG_WFPM_ACCESS: u32 = 0x0000_1000; // BIT(12)

// ── Poll timeouts (iwl-io.c / trans.c, microseconds) ─────────────
pub const HW_READY_TIMEOUT_US: u32 = 50;
pub const MAC_CLOCK_TIMEOUT_US: u32 = 25_000;

// ── Stage 1: RX/TX rings (iwl-csr.h / iwl-fh.h / fw/api/txq.h) ────
pub const CSR_INT_COALESCING: u32 = 0x004; // 32-usec units, u8 write
pub const IWL_HOST_INT_TIMEOUT_DEF: u8 = 0x40;
// Shadow-register enable mask written to CSR_MAC_SHADOW_REG_CTRL.
pub const CSR_MAC_SHADOW_REG_CTRL_VAL: u32 = 0x800F_FFFF;

// RX ring geometry. AX200 = mq_rx, family 22000 (< AX210), RF = HR.
pub const NUM_RBDS: usize = 256 * 8; // IWL_NUM_RBDS_HE (rf-hr.c)
pub const FREE_BD_SIZE: usize = 8; // __le64 RBD (mq, < AX210)
pub const USED_BD_SIZE: usize = 4; // __le32 (< AX210, < BZ)
pub const RB_STTS_SIZE: usize = 12; // sizeof(struct iwl_rb_status)

// TX command queue geometry (gen2).
pub const IWL_CMD_QUEUE_SIZE: usize = 32; // fw/api/txq.h
pub const TFH_TFD_SIZE: usize = 256; // sizeof(struct iwl_tfh_tfd)
pub const IWL_FIRST_TB_SIZE_ALIGN: usize = 64; // ALIGN(20, 64)

// ── Stage 2: context-info + FW load + ALIVE ──────────────────────
pub const CSR_CTXT_INFO_BA: u32 = 0x040; // 64-bit ctxt_info base address (kick)
pub const CSR_UCODE_DRV_GP1_CLR: u32 = 0x05C;
pub const CSR_UCODE_SW_BIT_RFKILL: u32 = 0x0000_0002;
pub const CSR_UCODE_DRV_GP1_BIT_CMD_BLOCKED: u32 = 0x0000_0004;
pub const CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW: u32 = 0x0800_0000;

// CSR_INT cause bits (iwl-csr.h)
pub const CSR_INT_BIT_ALIVE: u32 = 1 << 0; // uCode initialised
pub const CSR_INT_BIT_FH_RX: u32 = 1 << 31; // Rx DMA / cmd responses

// LTR boot workaround (iwl_pcie_set_ltr, 22000 non-integrated path)
pub const CSR_LTR_LONG_VAL_AD: u32 = 0x0D4;
pub const CSR_LTR_LONG_VAL_AD_NO_SNOOP_REQ: u32 = 0x8000_0000;
pub const CSR_LTR_LONG_VAL_AD_NO_SNOOP_SCALE: u32 = 0x1c00_0000;
pub const CSR_LTR_LONG_VAL_AD_NO_SNOOP_VAL: u32 = 0x03ff_0000;
pub const CSR_LTR_LONG_VAL_AD_SNOOP_REQ: u32 = 0x0000_8000;
pub const CSR_LTR_LONG_VAL_AD_SNOOP_SCALE: u32 = 0x0000_1c00;
pub const CSR_LTR_LONG_VAL_AD_SNOOP_VAL: u32 = 0x0000_03ff;
pub const CSR_LTR_LONG_VAL_AD_SCALE_USEC: u32 = 2;

// PRPH: tell the FW CPU to run (iwl-prph.h)
pub const UREG_CPU_INIT_RUN: u32 = 0x00A0_5C44;

// ── Firmware TLV format (fw/file.h) ──────────────────────────────
pub const FW_TLV_HEADER_LEN: usize = 88; // iwl_tlv_ucode_header
pub const IWL_UCODE_TLV_SEC_RT: u32 = 19; // regular runtime section
pub const CPU1_CPU2_SEPARATOR_SECTION: u32 = 0xFFFF_CCCC;
pub const PAGING_SEPARATOR_SECTION: u32 = 0xAAAA_BBBB;
pub const IWL_MAX_DRAM_ENTRY: usize = 64;

// ── Context-info struct (iwl-context-info.h), packed, 1792 bytes ──
pub const CTXT_INFO_SIZE: usize = 1792;
pub const CI_OFF_MAC_ID: usize = 0; // version.mac_id (u16)
pub const CI_OFF_VERSION: usize = 2; // version.version (u16)
pub const CI_OFF_SIZE: usize = 4; // version.size (u16, DWs)
pub const CI_OFF_CONTROL_FLAGS: usize = 8; // control.control_flags (u32)
pub const CI_OFF_FREE_RBD: usize = 24; // rbd_cfg.free_rbd_addr (u64)
pub const CI_OFF_USED_RBD: usize = 32; // rbd_cfg.used_rbd_addr (u64)
pub const CI_OFF_STATUS_WR: usize = 40; // rbd_cfg.status_wr_ptr (u64)
pub const CI_OFF_CMD_QUEUE_ADDR: usize = 48; // hcmd_cfg.cmd_queue_addr (u64)
pub const CI_OFF_CMD_QUEUE_SIZE: usize = 56; // hcmd_cfg.cmd_queue_size (u8)
pub const CI_OFF_UMAC_IMG: usize = 192; // dram.umac_img[64] (u64 each)
pub const CI_OFF_LMAC_IMG: usize = 704; // dram.lmac_img[64]
pub const CI_OFF_VIRTUAL_IMG: usize = 1216; // dram.virtual_img[64]

// control_flags fields (iwl_context_info_flags)
pub const IWL_CTXT_INFO_TFD_FORMAT_LONG: u32 = 0x0100;
pub const IWL_CTXT_INFO_RB_CB_SIZE_SHIFT: u32 = 4; // mask 0x00f0
pub const IWL_CTXT_INFO_RB_SIZE_SHIFT: u32 = 9; // mask 0x1e00
pub const IWL_CTXT_INFO_RB_SIZE_4K: u32 = 0x4; // default rx_buf_size
pub const CMD_QUEUE_CB_SIZE: u8 = 2; // TFD_QUEUE_CB_SIZE(32) = ilog2(32)-3

// ── Stage 3: RX restock + ALIVE notification ─────────────────────
// RFH free-RBD write-pointer trigger (direct MMIO in BAR0, gen2 < BZ).
pub const RFH_Q0_FRBDCB_WIDX_TRG: u32 = 0x1C80;
// RB pool size: enough to receive the alive notification (kept small to
// stay under MAX_DMA_ALLOCS; no npk_dma_free in the ABI). Each RB = 1 page.
pub const RX_NUM_RBS: usize = 64;
pub const RB_SIZE_BYTES: usize = 4096; // IWL_AMSDU_4K
// rb_stts.closed_rb_num producer index mask.
pub const RB_STTS_CLOSED_MASK: u32 = 0x0FFF;
// UCODE_ALIVE_NTFY command id (group 0).
pub const UCODE_ALIVE_NTFY: u8 = 0x01;

// ── PCIe capability layout (apm_config: ASPM / LTR detect) ───────
pub const PCI_CAP_PTR: u8 = 0x34; // first capability pointer
pub const PCI_CAP_ID_EXP: u8 = 0x10; // PCI Express capability
pub const PCI_EXP_LNKCTL: u8 = 0x10; // offset within PCIe cap
pub const PCI_EXP_DEVCTL2: u8 = 0x28; // offset within PCIe cap
pub const PCI_EXP_LNKCTL_ASPM_L0S: u16 = 0x0001;
pub const PCI_EXP_DEVCTL2_LTR_EN: u16 = 0x0400;
