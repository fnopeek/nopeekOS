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

// ── Stage 4a: ALIVE notification struct (fw/api/alive.h) ─────────
// The firmware DMAs a `struct iwl_alive_ntf_v6` (144 bytes) as the payload
// of an iwl_rx_packet. data[] starts after len_n_flags(4) + iwl_cmd_header(4).
pub const RX_PKT_DATA_OFF: usize = 8;
pub const IWL_ALIVE_STATUS_OK: u16 = 0xCAFE;
pub const IWL_ALIVE_STATUS_ERR: u16 = 0xDEAD;
// error_info_addr carries cache-control bits that must be masked off
// (fw/img.h: FW_ADDR_CACHE_CONTROL).
pub const FW_ADDR_CACHE_CONTROL: u32 = 0xC000_0000;
// Field offsets within iwl_alive_ntf_v6 (relative to the struct base):
//   __le16 status; __le16 flags; iwl_lmac_alive lmac_data[2];
//   iwl_umac_alive umac_data; iwl_sku_id sku_id; iwl_imr_alive_info imr;
pub const AL_OFF_STATUS: usize = 0; // __le16
pub const AL_OFF_LMAC0: usize = 4; // iwl_lmac_alive[0] (48 bytes)
pub const AL_OFF_UMAC: usize = 100; // 4 + 2*48
pub const AL_OFF_SKU_ID: usize = 116; // 4 + 2*48 + 16
// Within iwl_lmac_alive (48 bytes):
pub const LMAC_OFF_UCODE_MAJOR: usize = 0; // __le32
pub const LMAC_OFF_UCODE_MINOR: usize = 4; // __le32
pub const LMAC_OFF_VER_SUBTYPE: usize = 8; // u8
pub const LMAC_OFF_VER_TYPE: usize = 9; // u8
pub const LMAC_OFF_ERR_TABLE: usize = 16; // dbg_ptrs.error_event_table_ptr
// Within iwl_umac_alive (16 bytes):
pub const UMAC_OFF_MAJOR: usize = 0; // __le32
pub const UMAC_OFF_MINOR: usize = 4; // __le32
pub const UMAC_OFF_ERR_INFO: usize = 8; // dbg_ptrs.error_info_addr

// ── Stage 4b: host-command enqueue + init handshake ──────────────
// TX command-queue write-pointer doorbell (iwl-csr.h, HBUS_BASE 0x400 + 0x60).
pub const HBUS_TARG_WRPTR: u32 = 0x460;
// Command queue id = IWL_MVM_DQA_CMD_QUEUE (BUILD_BUG_ON forces it to 0).
pub const IWL_CMD_QUEUE_ID: u32 = 0;
// iwl_cmd_header_wide (fw/api/cmdhdr.h): cmd, group_id, __le16 sequence,
// __le16 length, reserved, version = 8 bytes.
pub const CMD_HDR_WIDE_LEN: usize = 8;
// IWL_FIRST_TB_SIZE (iwl-trans.h) — minimum bidirectional-DMA copy size; our
// whole init command fits within it (single TB).
pub const IWL_FIRST_TB_SIZE: usize = 20;
pub const HDRW_OFF_CMD: usize = 0;
pub const HDRW_OFF_GROUP: usize = 1;
pub const HDRW_OFF_SEQ: usize = 2; // __le16
pub const HDRW_OFF_LEN: usize = 4; // __le16 (payload length)
// iwl_txq_inc_wrap wraps at max_tfd_queue_size (TFD_QUEUE_SIZE_MAX = 256).
pub const MAX_TFD_QUEUE_SIZE: u32 = 256;
// iwl_tfh_tb: tb_len(__le16) + addr(__le64, unaligned) = 10 bytes. A TFD with a
// single TB needs num_tbs(__le16) + one tb = 12 bytes (rest of the 256 zeroed).
pub const TFD_SINGLE_TB_LEN: usize = 12;

// Command groups (fw/api/commands.h iwl_mvm_command_groups).
pub const LONG_GROUP: u8 = 0x1;
pub const SYSTEM_GROUP: u8 = 0x2;
pub const REGULATORY_AND_NVM_GROUP: u8 = 0xc;
// Init-flow command ids.
pub const INIT_EXTENDED_CFG_CMD: u8 = 0x03; // SYSTEM_GROUP (fw/api/commands.h)
pub const NVM_ACCESS_COMPLETE: u8 = 0x00; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)
pub const INIT_COMPLETE_NOTIF: u8 = 0x04; // legacy group 0 (commands.h)
// iwl_init_extended_cfg_cmd.init_flags = BIT(IWL_INIT_NVM); IWL_INIT_NVM = 1.
pub const IWL_INIT_NVM_FLAG: u32 = 1 << 1;

// RX used_bd entry is a bare __le32 for family < AX210; low 12 bits = VID.
pub const RX_VID_MASK: u32 = 0x0FFF;

// ── Stage 4c: NVM_GET_INFO (read channels/MAC/SKU) ───────────────
pub const NVM_GET_INFO: u8 = 0x02; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)
// iwl_rx_packet frame-size field (iwl-trans.h FH_RSCSR_FRAME_SIZE_MSK).
pub const FH_FRAME_SIZE_MASK: u32 = 0x0000_3FFF;
// MAC-address CSR registers. CSR_ADDR_BASE = base->mac_addr_from_csr = 0x380
// for family 22000 (cfg/22000.c). STRAP is the OEM-fused address; if invalid,
// fall back to the OTP address (iwl_set_hw_address_from_csr).
pub const CSR_MAC_ADDR0_STRAP: u32 = 0x388;
pub const CSR_MAC_ADDR1_STRAP: u32 = 0x38C;
pub const CSR_MAC_ADDR0_OTP: u32 = 0x380;
pub const CSR_MAC_ADDR1_OTP: u32 = 0x384;
// iwl_nvm_get_info_rsp field offsets (within the response payload). The
// general/mac_sku/phy_sku part is identical in the v3 and v4 responses.
pub const NVM_OFF_FLAGS: usize = 0; // general.flags __le32
pub const NVM_OFF_VERSION: usize = 4; // general.nvm_version __le16
pub const NVM_OFF_N_HW_ADDRS: usize = 7; // general.n_hw_addrs u8
pub const NVM_OFF_MAC_SKU: usize = 8; // mac_sku.mac_sku_flags __le32
pub const NVM_OFF_TX_CHAINS: usize = 12; // phy_sku.tx_chains __le32
pub const NVM_OFF_RX_CHAINS: usize = 16; // phy_sku.rx_chains __le32
pub const NVM_OFF_LAR: usize = 20; // regulatory.lar_enabled __le32
// mac_sku_flags bits (enum iwl_nvm_mac_sku_flags).
pub const NVM_SKU_BAND_24: u32 = 1 << 0;
pub const NVM_SKU_BAND_52: u32 = 1 << 1;
pub const NVM_SKU_11N: u32 = 1 << 2;
pub const NVM_SKU_11AC: u32 = 1 << 3;
pub const NVM_SKU_11AX: u32 = 1 << 4;

// ── Stage 4d: scan setup ─────────────────────────────────────────
// Firmware command-version TLV (fw/file.h): array of iwl_fw_cmd_version
// { u8 cmd; u8 group; u8 cmd_ver; u8 notif_ver; }. iwl_fw_lookup_cmd_ver maps
// (group ?: LONG_GROUP, cmd) → cmd_ver; this picks the versioned struct layouts
// the scan path needs (SCAN_REQ_UMAC etc. range over v1..v17).
pub const IWL_UCODE_TLV_CMD_VERSIONS: u32 = 48;
pub const IWL_FW_CMD_VER_UNKNOWN: u8 = 99;
pub const IWL_ALWAYS_LONG_GROUP: u8 = 1;
pub const FW_CMD_VER_ENTRY_LEN: usize = 4; // sizeof(iwl_fw_cmd_version)
// Command ids (fw/api/commands.h) for the scan path.
pub const PHY_CONTEXT_CMD: u8 = 0x08;
pub const SCAN_CFG_CMD: u8 = 0x0c;
pub const SCAN_REQ_UMAC: u8 = 0x0d;
pub const SCAN_COMPLETE_UMAC: u8 = 0x0f;
pub const ADD_STA: u8 = 0x18;
pub const MAC_CONTEXT_CMD: u8 = 0x28;
pub const TX_ANT_CONFIGURATION_CMD: u8 = 0x98;
pub const SCAN_ITERATION_COMPLETE_UMAC: u8 = 0xb5;
// Valid antenna mask (chains A+B). iwl_mvm_get_valid_tx_ant / iwl_mvm_scan_rx_ant
// resolve to fw->valid_tx/rx_ant & nvm = 0x3 on this 2×2 AX200 (Stage 4c caps).
pub const ANT_AB: u32 = 0x3;
// iwl_scan_config (SCAN_CFG v5, fw/api/scan.h): enable_cam_mode, enable_promisc,
// bcast_sta_id, reserved (all u8 → 0; v5 FW ignores bcast_sta_id), then
// __le32 tx_chains, __le32 rx_chains. 12 bytes total.
pub const SCAN_CFG_LEN: usize = 12;
pub const SCAN_CFG_OFF_TX_CHAINS: usize = 4;
pub const SCAN_CFG_OFF_RX_CHAINS: usize = 8;

// ── PCIe capability layout (apm_config: ASPM / LTR detect) ───────
pub const PCI_CAP_PTR: u8 = 0x34; // first capability pointer
pub const PCI_CAP_ID_EXP: u8 = 0x10; // PCI Express capability
pub const PCI_EXP_LNKCTL: u8 = 0x10; // offset within PCIe cap
pub const PCI_EXP_DEVCTL2: u8 = 0x28; // offset within PCIe cap
pub const PCI_EXP_LNKCTL_ASPM_L0S: u16 = 0x0001;
pub const PCI_EXP_DEVCTL2_LTR_EN: u16 = 0x0400;
