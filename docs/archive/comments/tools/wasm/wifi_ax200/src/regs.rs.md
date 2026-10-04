# `tools/wasm/wifi_ax200/src/regs.rs` @ 5e0102684

## L1-6 · `pub const AX200_VENDOR: u16 = 0x8086;`

```
//! AX200 register definitions — verified 1:1 against Linux 6.18.26
//! `drivers/net/wireless/intel/iwlwifi/iwl-csr.h` (CSR_BASE = 0x000).
//!
//! Only offsets verified against the header live here. Bit masks are added
//! per stage as each function is ported (strict 1:1, no guessed values —
//! see memory/feedback_linux_strict.md). BAR0 carries the CSR block.
```

## L8 · `pub const AX200_VENDOR: u16 = 0x8086;`

```
// ── PCI identity ─────────────────────────────────────────────────
```

## L10 · `pub const AX200_DEVICE: u16 = 0x2723; // iwl_ax200_mac_cfg, RF = HR, family 22000`

```
// iwl_ax200_mac_cfg, RF = HR, family 22000
```

## L12 · `pub const BAR_CSR: u8 = 0;`

```
// BAR carrying the CSR/PRPH register block (iwlwifi: BAR0).
```

## L15 · `pub const CSR_HW_IF_CONFIG_REG: u32 = 0x000; // hardware interface config`

```
// ── CSR registers (iwl-csr.h, CSR_BASE = 0x000) ──────────────────
```

## L16 · `pub const CSR_HW_IF_CONFIG_REG: u32 = 0x000; // hardware interface config`

```
// hardware interface config
```

## L17 · `pub const CSR_INT: u32 = 0x008; // host interrupt status/ack`

```
// host interrupt status/ack
```

## L18 · `pub const CSR_INT_MASK: u32 = 0x00C; // host interrupt enable`

```
// host interrupt enable
```

## L19 · `pub const CSR_FH_INT_STATUS: u32 = 0x010; // busmaster int status/ack`

```
// busmaster int status/ack
```

## L20 · `pub const CSR_RESET: u32 = 0x020; // busmaster enable, NMI, etc.`

```
// busmaster enable, NMI, etc.
```

## L23 · `pub const CSR_FUNC_SCRATCH: u32 = 0x02C; // FW debug scratch`

```
// FW debug scratch
```

## L33-34 · `pub const CSR_HW_IF_CONFIG_REG_HAP_WAKE: u32 = 0x0008_0000;`

```
// ── CSR bit masks (iwl-csr.h, verified) ──────────────────────────
// HW_IF_CONFIG
```

## L38 · `pub const CSR_MBOX_SET_REG_OS_ALIVE: u32 = 0x0000_0020; // BIT(5)`

```
// MBOX_SET
```

## L39 · `pub const CSR_MBOX_SET_REG_OS_ALIVE: u32 = 0x0000_0020; // BIT(5)`

```
// BIT(5)
```

## L40 · `pub const CSR_RESET_REG_FLAG_SW_RESET: u32 = 0x0000_0080;`

```
// RESET
```

## L43 · `pub const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 0x0000_0001;`

```
// GP_CNTRL
```

## L46 · `pub const CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX: u32 = 0x0080_0000;`

```
// GIO_CHICKEN / GIO / DBG_HPET
```

## L50 · `pub const CSR_HW_RF_ID_TYPE_HR: u32 = 0x0010_A000;`

```
// HW_RF_ID type (masked compare) — AX200 carries HR
```

## L53 · `pub const HBUS_TARG_PRPH_WADDR: u32 = 0x444;`

```
// ── PRPH access via HBUS (iwl-csr.h, HBUS_BASE = 0x400) ───────────
```

## L58 · `pub const PRPH_MASK: u32 = 0x000F_FFFF;`

```
// PRPH address mask for family < AX210 (iwl_trans_pcie_prph_msk)
```

## L61 · `pub const HPM_DEBUG: u32 = 0x00A0_3440;`

```
// ── PRPH registers / bits (iwl-prph.h) ───────────────────────────
```

## L63 · `pub const PERSISTENCE_BIT: u32 = 0x0000_1000; // BIT(12)`

```
// BIT(12)
```

## L65 · `pub const PREG_WFPM_ACCESS: u32 = 0x0000_1000; // BIT(12)`

```
// BIT(12)
```

## L67 · `pub const HW_READY_TIMEOUT_US: u32 = 50;`

```
// ── Poll timeouts (iwl-io.c / trans.c, microseconds) ─────────────
```

## L71 · `pub const CSR_INT_COALESCING: u32 = 0x004; // 32-usec units, u8 write`

```
// ── Stage 1: RX/TX rings (iwl-csr.h / iwl-fh.h / fw/api/txq.h) ────
```

## L72 · `pub const CSR_INT_COALESCING: u32 = 0x004; // 32-usec units, u8 write`

```
// 32-usec units, u8 write
```

## L74 · `pub const CSR_MAC_SHADOW_REG_CTRL_VAL: u32 = 0x800F_FFFF;`

```
// Shadow-register enable mask written to CSR_MAC_SHADOW_REG_CTRL.
```

## L77 · `pub const NUM_RBDS: usize = 256 * 8; // IWL_NUM_RBDS_HE (rf-hr.c)`

```
// RX ring geometry. AX200 = mq_rx, family 22000 (< AX210), RF = HR.
```

## L78 · `pub const NUM_RBDS: usize = 256 * 8; // IWL_NUM_RBDS_HE (rf-hr.c)`

```
// IWL_NUM_RBDS_HE (rf-hr.c)
```

## L79 · `pub const FREE_BD_SIZE: usize = 8; // __le64 RBD (mq, < AX210)`

```
// __le64 RBD (mq, < AX210)
```

## L80 · `pub const USED_BD_SIZE: usize = 4; // __le32 (< AX210, < BZ)`

```
// __le32 (< AX210, < BZ)
```

## L81 · `pub const RB_STTS_SIZE: usize = 12; // sizeof(struct iwl_rb_status)`

```
// sizeof(struct iwl_rb_status)
```

## L83 · `pub const IWL_CMD_QUEUE_SIZE: usize = 32; // fw/api/txq.h`

```
// TX command queue geometry (gen2).
```

## L84 · `pub const IWL_CMD_QUEUE_SIZE: usize = 32; // fw/api/txq.h`

```
// fw/api/txq.h
```

## L85 · `pub const TFH_TFD_SIZE: usize = 256; // sizeof(struct iwl_tfh_tfd)`

```
// sizeof(struct iwl_tfh_tfd)
```

## L86 · `pub const IWL_FIRST_TB_SIZE_ALIGN: usize = 64; // ALIGN(20, 64)`

```
// ALIGN(20, 64)
```

## L88 · `pub const CSR_CTXT_INFO_BA: u32 = 0x040; // 64-bit ctxt_info base address (kick)`

```
// ── Stage 2: context-info + FW load + ALIVE ──────────────────────
```

## L89 · `pub const CSR_CTXT_INFO_BA: u32 = 0x040; // 64-bit ctxt_info base address (kick)`

```
// 64-bit ctxt_info base address (kick)
```

## L95 · `pub const CSR_INT_BIT_ALIVE: u32 = 1 << 0; // uCode initialised`

```
// CSR_INT cause bits (iwl-csr.h)
```

## L96 · `pub const CSR_INT_BIT_ALIVE: u32 = 1 << 0; // uCode initialised`

```
// uCode initialised
```

## L97 · `pub const CSR_INT_BIT_FH_RX: u32 = 1 << 31; // Rx DMA / cmd responses`

```
// Rx DMA / cmd responses
```

## L99 · `pub const CSR_LTR_LONG_VAL_AD: u32 = 0x0D4;`

```
// LTR boot workaround (iwl_pcie_set_ltr, 22000 non-integrated path)
```

## L109 · `pub const UREG_CPU_INIT_RUN: u32 = 0x00A0_5C44;`

```
// PRPH: tell the FW CPU to run (iwl-prph.h)
```

## L112 · `pub const FW_TLV_HEADER_LEN: usize = 88; // iwl_tlv_ucode_header`

```
// ── Firmware TLV format (fw/file.h) ──────────────────────────────
```

## L113 · `pub const FW_TLV_HEADER_LEN: usize = 88; // iwl_tlv_ucode_header`

```
// iwl_tlv_ucode_header
```

## L114 · `pub const IWL_UCODE_TLV_SEC_RT: u32 = 19; // regular runtime section`

```
// regular runtime section
```

## L119 · `pub const CTXT_INFO_SIZE: usize = 1792;`

```
// ── Context-info struct (iwl-context-info.h), packed, 1792 bytes ──
```

## L121 · `pub const CI_OFF_MAC_ID: usize = 0; // version.mac_id (u16)`

```
// version.mac_id (u16)
```

## L122 · `pub const CI_OFF_VERSION: usize = 2; // version.version (u16)`

```
// version.version (u16)
```

## L123 · `pub const CI_OFF_SIZE: usize = 4; // version.size (u16, DWs)`

```
// version.size (u16, DWs)
```

## L124 · `pub const CI_OFF_CONTROL_FLAGS: usize = 8; // control.control_flags (u32)`

```
// control.control_flags (u32)
```

## L125 · `pub const CI_OFF_FREE_RBD: usize = 24; // rbd_cfg.free_rbd_addr (u64)`

```
// rbd_cfg.free_rbd_addr (u64)
```

## L126 · `pub const CI_OFF_USED_RBD: usize = 32; // rbd_cfg.used_rbd_addr (u64)`

```
// rbd_cfg.used_rbd_addr (u64)
```

## L127 · `pub const CI_OFF_STATUS_WR: usize = 40; // rbd_cfg.status_wr_ptr (u64)`

```
// rbd_cfg.status_wr_ptr (u64)
```

## L128 · `pub const CI_OFF_CMD_QUEUE_ADDR: usize = 48; // hcmd_cfg.cmd_queue_addr (u64)`

```
// hcmd_cfg.cmd_queue_addr (u64)
```

## L129 · `pub const CI_OFF_CMD_QUEUE_SIZE: usize = 56; // hcmd_cfg.cmd_queue_size (u8)`

```
// hcmd_cfg.cmd_queue_size (u8)
```

## L130 · `pub const CI_OFF_UMAC_IMG: usize = 192; // dram.umac_img[64] (u64 each)`

```
// dram.umac_img[64] (u64 each)
```

## L131 · `pub const CI_OFF_LMAC_IMG: usize = 704; // dram.lmac_img[64]`

```
// dram.lmac_img[64]
```

## L132 · `pub const CI_OFF_VIRTUAL_IMG: usize = 1216; // dram.virtual_img[64]`

```
// dram.virtual_img[64]
```

## L134 · `pub const IWL_CTXT_INFO_TFD_FORMAT_LONG: u32 = 0x0100;`

```
// control_flags fields (iwl_context_info_flags)
```

## L136 · `pub const IWL_CTXT_INFO_RB_CB_SIZE_SHIFT: u32 = 4; // mask 0x00f0`

```
// mask 0x00f0
```

## L137 · `pub const IWL_CTXT_INFO_RB_SIZE_SHIFT: u32 = 9; // mask 0x1e00`

```
// mask 0x1e00
```

## L138 · `pub const IWL_CTXT_INFO_RB_SIZE_4K: u32 = 0x4; // default rx_buf_size`

```
// default rx_buf_size
```

## L139 · `pub const CMD_QUEUE_CB_SIZE: u8 = 2; // TFD_QUEUE_CB_SIZE(32) = ilog2(32)-3`

```
// TFD_QUEUE_CB_SIZE(32) = ilog2(32)-3
```

## L141-142 · `pub const RFH_Q0_FRBDCB_WIDX_TRG: u32 = 0x1C80;`

```
// ── Stage 3: RX restock + ALIVE notification ─────────────────────
// RFH free-RBD write-pointer trigger (direct MMIO in BAR0, gen2 < BZ).
```

## L144-156 · `pub const RX_NUM_RBS: usize = 512;`

```
// RB pool size. Three numbers have stood here: 64 ("enough for the alive
// notification"), 256, then 512. The last one took the link down, so it is out
// again — 256 is the only value this driver has been MEASURED at (99 Mbit,
// drain-peak 241/256). Each RB = 1 page.
//
// Linux does not pick this number, it derives it: the pool is
// `trans_pcie->num_rx_bufs - 1` = NUM_RBDS - 1 = 2047 buffers in a 2048-slot
// ring (pcie/gen1_2/rx.c, iwl_pcie_rx_init). The -1 is the ring rule, spelled
// out in rx.c:126 — write == read must mean EMPTY, so N slots can hold at most
// N-1 entries. Raising this towards 2047 is a throughput question and needs a
// device measurement per step plus room in the kernel's MAX_DMA_ALLOCS /
// MAX_DMA_PAGES; it is no longer a correctness question, because the free-BD
// ring now tracks buffer ownership instead of trusting the pool to be small.
```

## L158-159 · `const _: () = assert!(RX_NUM_RBS < NUM_RBDS);`

```
// The ring rule, machine-checked instead of remembered: Linux itself stops one
// short of the ring (num_rbds - 1 = 2047 for this chip).
```

## L161 · `const _: () = assert!(RX_NUM_RBS < NUM_RBDS);`

```
// The ring rule, machine-checked instead of remembered.
```

## L163 · `pub const RB_SIZE_BYTES: usize = 4096; // IWL_AMSDU_4K`

```
// IWL_AMSDU_4K
```

## L164 · `pub const RB_STTS_CLOSED_MASK: u32 = 0x0FFF;`

```
// rb_stts.closed_rb_num producer index mask.
```

## L166 · `pub const UCODE_ALIVE_NTFY: u8 = 0x01;`

```
// UCODE_ALIVE_NTFY command id (group 0).
```

## L169-171 · `pub const RX_PKT_DATA_OFF: usize = 8;`

```
// ── Stage 4a: ALIVE notification struct (fw/api/alive.h) ─────────
// The firmware DMAs a `struct iwl_alive_ntf_v6` (144 bytes) as the payload
// of an iwl_rx_packet. data[] starts after len_n_flags(4) + iwl_cmd_header(4).
```

## L175-176 · `pub const FW_ADDR_CACHE_CONTROL: u32 = 0xC000_0000;`

```
// error_info_addr carries cache-control bits that must be masked off
// (fw/img.h: FW_ADDR_CACHE_CONTROL).
```

## L178-180 · `pub const AL_OFF_STATUS: usize = 0; // __le16`

```
// Field offsets within iwl_alive_ntf_v6 (relative to the struct base):
//   __le16 status; __le16 flags; iwl_lmac_alive lmac_data[2];
//   iwl_umac_alive umac_data; iwl_sku_id sku_id; iwl_imr_alive_info imr;
```

## L181 · `pub const AL_OFF_STATUS: usize = 0; // __le16`

```
// __le16
```

## L182 · `pub const AL_OFF_LMAC0: usize = 4; // iwl_lmac_alive[0] (48 bytes)`

```
// iwl_lmac_alive[0] (48 bytes)
```

## L183 · `pub const AL_OFF_UMAC: usize = 100; // 4 + 2*48`

```
// 4 + 2*48
```

## L184 · `pub const AL_OFF_SKU_ID: usize = 116; // 4 + 2*48 + 16`

```
// 4 + 2*48 + 16
```

## L185 · `pub const LMAC_OFF_UCODE_MAJOR: usize = 0; // __le32`

```
// Within iwl_lmac_alive (48 bytes):
```

## L186 · `pub const LMAC_OFF_UCODE_MAJOR: usize = 0; // __le32`

```
// __le32
```

## L187 · `pub const LMAC_OFF_UCODE_MINOR: usize = 4; // __le32`

```
// __le32
```

## L188 · `pub const LMAC_OFF_VER_SUBTYPE: usize = 8; // u8`

```
// u8
```

## L189 · `pub const LMAC_OFF_VER_TYPE: usize = 9; // u8`

```
// u8
```

## L190 · `pub const LMAC_OFF_ERR_TABLE: usize = 16; // dbg_ptrs.error_event_table_ptr`

```
// dbg_ptrs.error_event_table_ptr
```

## L191 · `pub const UMAC_OFF_MAJOR: usize = 0; // __le32`

```
// Within iwl_umac_alive (16 bytes):
```

## L192 · `pub const UMAC_OFF_MAJOR: usize = 0; // __le32`

```
// __le32
```

## L193 · `pub const UMAC_OFF_MINOR: usize = 4; // __le32`

```
// __le32
```

## L194 · `pub const UMAC_OFF_ERR_INFO: usize = 8; // dbg_ptrs.error_info_addr`

```
// dbg_ptrs.error_info_addr
```

## L196-197 · `pub const HBUS_TARG_WRPTR: u32 = 0x460;`

```
// ── Stage 4b: host-command enqueue + init handshake ──────────────
// TX command-queue write-pointer doorbell (iwl-csr.h, HBUS_BASE 0x400 + 0x60).
```

## L199 · `pub const IWL_CMD_QUEUE_ID: u32 = 0;`

```
// Command queue id = IWL_MVM_DQA_CMD_QUEUE (BUILD_BUG_ON forces it to 0).
```

## L201-202 · `pub const CMD_HDR_WIDE_LEN: usize = 8;`

```
// iwl_cmd_header_wide (fw/api/cmdhdr.h): cmd, group_id, __le16 sequence,
// __le16 length, reserved, version = 8 bytes.
```

## L204-205 · `pub const IWL_FIRST_TB_SIZE: usize = 20;`

```
// IWL_FIRST_TB_SIZE (iwl-trans.h) — minimum bidirectional-DMA copy size; our
// whole init command fits within it (single TB).
```

## L209 · `pub const HDRW_OFF_SEQ: usize = 2; // __le16`

```
// __le16
```

## L210 · `pub const HDRW_OFF_LEN: usize = 4; // __le16 (payload length)`

```
// __le16 (payload length)
```

## L211 · `pub const MAX_TFD_QUEUE_SIZE: u32 = 256;`

```
// iwl_txq_inc_wrap wraps at max_tfd_queue_size (TFD_QUEUE_SIZE_MAX = 256).
```

## L213 · `pub const TFH_TB_LEN: usize = 10;`

```
// iwl_tfh_tb: tb_len(__le16) + addr(__le64, unaligned) = 10 bytes.
```

## L215-216 · `pub const FIRST_TB_HEAD_MAX: usize = IWL_FIRST_TB_SIZE - CMD_HDR_WIDE_LEN;`

```
// Max payload bytes that ride in the first-TB staging buffer alongside the
// 8-byte wide header (IWL_FIRST_TB_SIZE - CMD_HDR_WIDE_LEN = 20 - 8).
```

## L218-219 · `pub const CMD_DATA_BYTES: usize = 4096;`

```
// cmd_data buffer: holds the bulk of a large host command (mapped as TB1).
// One page covers SCAN_REQ_UMAC (~1.7 KB).
```

## L222 · `pub const LONG_GROUP: u8 = 0x1;`

```
// Command groups (fw/api/commands.h iwl_mvm_command_groups).
```

## L226 · `pub const INIT_EXTENDED_CFG_CMD: u8 = 0x03; // SYSTEM_GROUP (fw/api/commands.h)`

```
// Init-flow command ids.
```

## L227 · `pub const INIT_EXTENDED_CFG_CMD: u8 = 0x03; // SYSTEM_GROUP (fw/api/commands.h)`

```
// SYSTEM_GROUP (fw/api/commands.h)
```

## L228 · `pub const NVM_ACCESS_COMPLETE: u8 = 0x00; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)`

```
// REGULATORY_AND_NVM_GROUP (nvm-reg.h)
```

## L229 · `pub const INIT_COMPLETE_NOTIF: u8 = 0x04; // legacy group 0 (commands.h)`

```
// legacy group 0 (commands.h)
```

## L230 · `pub const IWL_INIT_NVM_FLAG: u32 = 1 << 1;`

```
// iwl_init_extended_cfg_cmd.init_flags = BIT(IWL_INIT_NVM); IWL_INIT_NVM = 1.
```

## L233 · `pub const RX_VID_MASK: u32 = 0x0FFF;`

```
// RX used_bd entry is a bare __le32 for family < AX210; low 12 bits = VID.
```

## L236 · `pub const NVM_GET_INFO: u8 = 0x02; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)`

```
// ── Stage 4c: NVM_GET_INFO (read channels/MAC/SKU) ───────────────
```

## L237 · `pub const NVM_GET_INFO: u8 = 0x02; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)`

```
// REGULATORY_AND_NVM_GROUP (nvm-reg.h)
```

## L238 · `pub const FH_FRAME_SIZE_MASK: u32 = 0x0000_3FFF;`

```
// iwl_rx_packet frame-size field (iwl-trans.h FH_RSCSR_FRAME_SIZE_MSK).
```

## L240-242 · `pub const CSR_MAC_ADDR0_STRAP: u32 = 0x388;`

```
// MAC-address CSR registers. CSR_ADDR_BASE = base->mac_addr_from_csr = 0x380
// for family 22000 (cfg/22000.c). STRAP is the OEM-fused address; if invalid,
// fall back to the OTP address (iwl_set_hw_address_from_csr).
```

## L247-248 · `pub const NVM_OFF_FLAGS: usize = 0; // general.flags __le32`

```
// iwl_nvm_get_info_rsp field offsets (within the response payload). The
// general/mac_sku/phy_sku part is identical in the v3 and v4 responses.
```

## L249 · `pub const NVM_OFF_FLAGS: usize = 0; // general.flags __le32`

```
// general.flags __le32
```

## L250 · `pub const NVM_OFF_VERSION: usize = 4; // general.nvm_version __le16`

```
// general.nvm_version __le16
```

## L251 · `pub const NVM_OFF_N_HW_ADDRS: usize = 7; // general.n_hw_addrs u8`

```
// general.n_hw_addrs u8
```

## L252 · `pub const NVM_OFF_MAC_SKU: usize = 8; // mac_sku.mac_sku_flags __le32`

```
// mac_sku.mac_sku_flags __le32
```

## L253 · `pub const NVM_OFF_TX_CHAINS: usize = 12; // phy_sku.tx_chains __le32`

```
// phy_sku.tx_chains __le32
```

## L254 · `pub const NVM_OFF_RX_CHAINS: usize = 16; // phy_sku.rx_chains __le32`

```
// phy_sku.rx_chains __le32
```

## L255 · `pub const NVM_OFF_LAR: usize = 20; // regulatory.lar_enabled __le32`

```
// regulatory.lar_enabled __le32
```

## L256 · `pub const NVM_SKU_BAND_24: u32 = 1 << 0;`

```
// mac_sku_flags bits (enum iwl_nvm_mac_sku_flags).
```

## L263-265 · `pub const NVM_OFF_CHANNEL_PROFILE: usize = 28;`

```
// NVM_GET_INFO v4 regulatory section: per-channel flags. lar_enabled @20,
// n_channels @24, then channel_profile (__le32 per channel) @28 within the
// response payload (iwl_nvm_get_info_rsp, REGULATORY_NVM_GET_INFO_RSP_API_S_VER_4).
```

## L267 · `pub const NVM_CHANNEL_VALID: u32 = 1 << 0;`

```
// iwl_nvm_channel_flags (iwl-nvm-parse.h): bit 0 = usable for this SKU/geo.
```

## L269-270 · `pub const NVM_NUM_2GHZ: usize = 14;`

```
// AX200 is RF-HR → cfg.nvm_type = IWL_NVM_EXT, not uhb → iwl_ext_nvm_channels:
// 14 × 2.4 GHz then 37 × 5 GHz (iwl_nl80211_band_from_channel_idx splits at 14).
```

## L274 · `1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,`

```
// 2.4 GHz
```

## L276 · `36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92,`

```
// 5 GHz
```

## L282-286 · `pub const IWL_UCODE_TLV_CMD_VERSIONS: u32 = 48;`

```
// ── Stage 4d: scan setup ─────────────────────────────────────────
// Firmware command-version TLV (fw/file.h): array of iwl_fw_cmd_version
// { u8 cmd; u8 group; u8 cmd_ver; u8 notif_ver; }. iwl_fw_lookup_cmd_ver maps
// (group ?: LONG_GROUP, cmd) → cmd_ver; this picks the versioned struct layouts
// the scan path needs (SCAN_REQ_UMAC etc. range over v1..v17).
```

## L290 · `pub const FW_CMD_VER_ENTRY_LEN: usize = 4; // sizeof(iwl_fw_cmd_version)`

```
// sizeof(iwl_fw_cmd_version)
```

## L291 · `pub const PHY_CONTEXT_CMD: u8 = 0x08;`

```
// Command ids (fw/api/commands.h) for the scan path.
```

## L300-301 · `pub const ANT_AB: u32 = 0x3;`

```
// Valid antenna mask (chains A+B). iwl_mvm_get_valid_tx_ant / iwl_mvm_scan_rx_ant
// resolve to fw->valid_tx/rx_ant & nvm = 0x3 on this 2×2 AX200 (Stage 4c caps).
```

## L304-307 · `pub const SCAN_CMD_LEN: usize = 1940;`

```
// ── Stage 4d2b: SCAN_REQ_UMAC v17 (passive regular scan) ─────────
// Total size of iwl_scan_req_umac_v17 (uid 4 + ooc 4 + general 36 + channel
// 4+67*8 + periodic 12 + probe 1344). probe_params stays zeroed (passive scan
// transmits no probe request, so its preq is unused).
```

## L309 · `pub const SC_OFF_OOC_PRIORITY: usize = 4; // uid @ 0 stays 0`

```
// iwl_scan_req_umac_v17 field offsets:
```

## L310 · `pub const SC_OFF_OOC_PRIORITY: usize = 4; // uid @ 0 stays 0`

```
// uid @ 0 stays 0
```

## L311 · `pub const SC_OFF_GP_FLAGS: usize = 8; // __le16`

```
// general_params_v11 @ 8 (36 bytes):
```

## L312 · `pub const SC_OFF_GP_FLAGS: usize = 8; // __le16`

```
// __le16
```

## L313 · `pub const SC_OFF_GP_ACTIVE_DWELL: usize = 12; // u8[2] (LB, HB)`

```
// u8[2] (LB, HB)
```

## L317 · `pub const SC_OFF_GP_FLAGS2: usize = 17; // 0 (non-cdb, regular)`

```
// 0 (non-cdb, regular)
```

## L318 · `pub const SC_OFF_GP_ADWELL_BUDGET: usize = 18; // __le16`

```
// __le16
```

## L319 · `pub const SC_OFF_GP_SCAN_PRIO: usize = 36; // __le32`

```
// max_out_of_time[2] @ 20, suspend_time[2] @ 28 — all 0 for UNASSOC type.
```

## L320 · `pub const SC_OFF_GP_SCAN_PRIO: usize = 36; // __le32`

```
// __le32
```

## L321 · `pub const SC_OFF_GP_PASSIVE_DWELL: usize = 40; // u8[2]`

```
// u8[2]
```

## L322-323 · `pub const SC_OFF_CP_FLAGS: usize = 44; // u8`

```
// num_of_fragments[2] @ 42 — 0 (not fragmented).
// channel_params_v7 @ 44:
```

## L324 · `pub const SC_OFF_CP_FLAGS: usize = 44; // u8`

```
// u8
```

## L325 · `pub const SC_OFF_CP_COUNT: usize = 45; // u8`

```
// u8
```

## L326 · `pub const SC_OFF_CP_N_APS_OVERRIDE: usize = 46; // u8[2]`

```
// u8[2]
```

## L327 · `pub const SC_OFF_CP_CHANNELS: usize = 48; // iwl_scan_channel_cfg_umac[67]`

```
// iwl_scan_channel_cfg_umac[67]
```

## L328 · `pub const SCAN_CH_CFG_LEN: usize = 8; // {__le32 flags, u8 channel_num, union[3]}`

```
// {__le32 flags, u8 channel_num, union[3]}
```

## L329 · `pub const SC_OFF_PERIODIC_SCHED0_ITER: usize = 584 + 2; // schedule[0].iter_count`

```
// periodic_params_v1 @ 584: schedule[2] (4 B each), delay, reserved.
```

## L330 · `pub const SC_OFF_PERIODIC_SCHED0_ITER: usize = 584 + 2; // schedule[0].iter_count`

```
// schedule[0].iter_count
```

## L332 · `pub const SCAN_OOC_PRIORITY_REGULAR: u32 = 6; // IWL_SCAN_PRIORITY_EXT_6`

```
// Values (mvm/scan.c):
```

## L333 · `pub const SCAN_OOC_PRIORITY_REGULAR: u32 = 6; // IWL_SCAN_PRIORITY_EXT_6`

```
// IWL_SCAN_PRIORITY_EXT_6
```

## L334 · `pub const SCAN_GP_FLAGS_PASSIVE: u16 = (1 << 11) | (1 << 1) | (1 << 7);`

```
// gen flags v2: FORCE_PASSIVE BIT(11) | PASS_ALL BIT(1) | ADAPTIVE_DWELL BIT(7).
```

## L344 · `pub const SCAN_CHAN_FLAG_ENABLE_CHAN_ORDER: u8 = 1 << 5; // BIT(5)`

```
// BIT(5)
```

## L345 · `pub const PHY_BAND_24: u32 = 1; // phy-ctxt.h`

```
// phy-ctxt.h
```

## L346 · `pub const PHY_BAND_5: u32 = 0; // phy-ctxt.h`

```
// phy-ctxt.h
```

## L348 · `pub const SCAN_MAX_CHANS: usize = 67;`

```
// Upper bound on channels we put in one scan: command holds SCAN_MAX_NUM_CHANS_V3.
```

## L350-352 · `pub const SCAN_CFG_LEN: usize = 12;`

```
// iwl_scan_config (SCAN_CFG v5, fw/api/scan.h): enable_cam_mode, enable_promisc,
// bcast_sta_id, reserved (all u8 → 0; v5 FW ignores bcast_sta_id), then
// __le32 tx_chains, __le32 rx_chains. 12 bytes total.
```

## L356-359 · `pub const SC_OFF_GP_SCAN_START_MAC: usize = 11;`

```
// general_params.scan_start_mac_or_link_id (u8 @ general+3 → cmd offset 11).
// It must name the firmware MAC context the scan runs on. For SCAN_REQ_UMAC
// version < 16 this is scan_vif->id (mvm/scan.c iwl_mvm_scan_umac_fill_general_
// p_v12); our station vif gets MAC id 0 (see Stage 4d2b1b below).
```

## L362-369 · `pub const MAC_CONTEXT_CMD_OP: u8 = 0x28;`

```
// ── Stage 4d2b1b: MAC context (iwl_mac_ctx_cmd, fw/api/mac.h) ─────
// A scan references a firmware MAC context (scan_start_mac_or_link_id).
// mac80211 creates it on add_interface (iwl_mvm_mac_ctxt_add); our driver-
// initiated scan must add it first. NOTE: no aux station / TX queue is added
// — for ADD_STA cmd_ver >= 12 (this firmware) iwl_mvm_has_new_station_api is
// true, so iwl_mvm_up does NOT call iwl_mvm_add_aux_sta; the firmware uses an
// internal aux station for scan activity (mvm/fw.c:1459, mvm/mvm.h:1509).
// MAC_CONTEXT_CMD is opcode 0x28 in the legacy group (group 0).
```

## L371-372 · `pub const MAC_CTX_CMD_LEN: usize = 148;`

```
// sizeof(struct iwl_mac_ctx_cmd): 100-byte common+qos+ac[AC_NUM+1=5] + a
// 48-byte union (largest member p2p_sta). We fill only the 44-byte .sta.
```

## L374 · `pub const FW_CTXT_ACTION_ADD: u32 = 1; // enum iwl_ctxt_action (INVALID=0,ADD=1)`

```
// enum iwl_ctxt_action (INVALID=0,ADD=1)
```

## L375 · `pub const FW_CTXT_ACTION_MODIFY: u32 = 2; // enum iwl_ctxt_action (MODIFY=2)`

```
// enum iwl_ctxt_action (MODIFY=2)
```

## L376 · `pub const FW_MAC_TYPE_BSS_STA: u32 = 5; // enum iwl_mac_types`

```
// enum iwl_mac_types
```

## L377 · `pub const SCAN_VIF_MAC_ID: u8 = 0; // ctxt_init id for the 1st non-p2p station`

```
// ctxt_init id for the 1st non-p2p station
```

## L378 · `pub const MC_OFF_ID_COLOR: usize = 0; // __le32 FW_CMD_ID_AND_COLOR(0,0)=0`

```
// iwl_mac_ctx_cmd field offsets (packed):
```

## L379 · `pub const MC_OFF_ID_COLOR: usize = 0; // __le32 FW_CMD_ID_AND_COLOR(0,0)=0`

```
// __le32 FW_CMD_ID_AND_COLOR(0,0)=0
```

## L380 · `pub const MC_OFF_ACTION: usize = 4; // __le32`

```
// __le32
```

## L381 · `pub const MC_OFF_MAC_TYPE: usize = 8; // __le32`

```
// __le32
```

## L382 · `pub const MC_OFF_TSF_ID: usize = 12; // __le32 (TSF_ID_A = 0)`

```
// __le32 (TSF_ID_A = 0)
```

## L383 · `pub const MC_OFF_NODE_ADDR: usize = 16; // u8[6] (+ __le16 reserved @ 22)`

```
// u8[6] (+ __le16 reserved @ 22)
```

## L384 · `pub const MC_OFF_BSSID_ADDR: usize = 24; // u8[6] (+ __le16 reserved @ 30)`

```
// u8[6] (+ __le16 reserved @ 30)
```

## L385 · `pub const MC_OFF_CCK_RATES: usize = 32; // __le32`

```
// __le32
```

## L386 · `pub const MC_OFF_OFDM_RATES: usize = 36; // __le32`

```
// __le32
```

## L387 · `pub const MC_OFF_PROT_FLAGS: usize = 40; // __le32 (enum iwl_mac_protection_flags)`

```
// __le32 (enum iwl_mac_protection_flags)
```

## L388 · `pub const MAC_PROT_FLG_TGG_PROTECT: u32 = 1 << 3;`

```
/// fw/api/mac.h:40 — what the firmware protects transmissions with.
```

## L392 · `pub const MC_OFF_FILTER_FLAGS: usize = 52; // __le32`

```
// __le32
```

## L393-397 · `pub const MC_OFF_QOS_FLAGS: usize = 56;  // __le32`

```
// ── MAC_QOS_PARAM_API_S_VER_1, inside struct iwl_mac_ctx_cmd ──────
// We left both of these at zero since the first port. That tells the firmware
// there is no EDCA configuration and this is not an 802.11n BSS — and a
// firmware that does not know it is in an HT/QoS BSS has no reason to open a
// TX aggregation session. `iwl_mvm_set_fw_qos_params` (mvm/mac-ctxt.c:475).
```

## L398 · `pub const MC_OFF_QOS_FLAGS: usize = 56;  // __le32`

```
// __le32
```

## L399 · `pub const MC_OFF_AC: usize = 60;         // struct iwl_ac_qos ac[AC_NUM + 1]`

```
// struct iwl_ac_qos ac[AC_NUM + 1]
```

## L401 · `pub const ACQ_OFF_CW_MIN: usize = 0;     // __le16`

```
// __le16
```

## L402 · `pub const ACQ_OFF_CW_MAX: usize = 2;     // __le16`

```
// __le16
```

## L403 · `pub const ACQ_OFF_AIFSN: usize = 4;      // u8`

```
// u8
```

## L404 · `pub const ACQ_OFF_FIFOS_MASK: usize = 5; // u8`

```
// u8
```

## L405 · `pub const ACQ_OFF_EDCA_TXOP: usize = 6;  // __le16, microseconds`

```
// __le16, microseconds
```

## L408-411 · `pub const AC_TO_UCODE_AC: [usize; 4] = [3, 2, 1, 0]; // AC_VO, AC_VI, AC_BE, AC_BK`

```
// enum iwl_ac (fw/api/mac.h:22) and enum iwl_mvm_tx_fifo (fw/api/txq.h:48),
// both indexed by the mac80211 AC number (VO, VI, BE, BK) exactly as
// `mac80211_ac_to_ucode_ac` (mvm/utils.c:175) and `iwl_mvm_ac_to_tx_fifo`
// (mvm/mac-ctxt.c:17) are.
```

## L412 · `pub const AC_TO_UCODE_AC: [usize; 4] = [3, 2, 1, 0]; // AC_VO, AC_VI, AC_BE, AC_BK`

```
// AC_VO, AC_VI, AC_BE, AC_BK
```

## L413-424 · `pub const AC_TO_TX_FIFO: [u8; 4] = [4, 3, 2, 1];     // VO, VI, BE, BK (gen2)`

```
// `iwl_mvm_mac_ac_to_tx_fifo` (mvm/mvm.h) picks between THREE tables, and the
// one printed at mac-ctxt.c:17 is the LEGACY one:
//
//     if (device_family >= IWL_DEVICE_FAMILY_BZ) return iwl_mvm_ac_to_bz_tx_fifo[ac];
//     if (iwl_mvm_has_new_tx_api(mvm))           return iwl_mvm_ac_to_gen2_tx_fifo[ac];
//     return iwl_mvm_ac_to_tx_fifo[ac];
//
// We are 22000 with the new TX API, so it is the GEN2 table — and gen2
// numbers the FIFOs differently (`enum iwl_gen2_tx_fifo`, fw/api/txq.h:57):
// CMD = 0, EDCA_BK = 1, EDCA_BE = 2, EDCA_VI = 3, EDCA_VO = 4.
// 0.95.0 shipped the legacy [3,2,1,0]: BE landed in BK's FIFO and BK in the
// COMMAND FIFO. Measured: `blocked` 45 -> 21570, `tx drops full` 0 -> 7569.
```

## L425 · `pub const AC_TO_TX_FIFO: [u8; 4] = [4, 3, 2, 1];     // VO, VI, BE, BK (gen2)`

```
// VO, VI, BE, BK (gen2)
```

## L426-429 · `pub const STA_MODIFY_TID_DISABLE_TX: u8 = 1 << 1;`

```
// Aggregation-manager experiment. Linux keeps tid_disable_tx at 0xffff on
// TLC-offload firmware (mac80211 never opens a TX BA session, so nothing ever
// clears it). Three attempts have not produced a single aggregate, so this is
// the switch that tests the field itself instead of arguing about it.
```

## L433-435 · `pub const WMM_OUI: [u8; 3] = [0x00, 0x50, 0xf2];`

```
// ── WMM Parameter element (vendor-specific 221) ──────────────────
// OUI 00:50:F2, type 2, subtype 1 is the PARAMETER element (subtype 0 is the
// shorter Information element, which carries no EDCA table).
```

## L440 · `pub const WMM_PARAM_OFF_AC: usize = 8; // after oui(3) type subtype version qos_info reserved`

```
// after oui(3) type subtype version qos_info reserved
```

## L442-445 · `pub const MC_OFF_STA_IS_ASSOC: usize = 100; // __le32 (0 = not yet associated)`

```
// cck_short_preamble @ 44, short_slot @ 48, qos_flags @ 56, ac[5] @ 60 — all 0.
// union iwl_mac_data_sta @ 100 (after qos_flags @56 + ac[AC_NUM+1=5]*8 = 40).
// For the connect MODIFY (iwl_mvm_mac_ctxt_cmd_sta, unassoc branch) we set the
// timing fields the firmware needs to schedule the auth/assoc on the target BSS.
```

## L446 · `pub const MC_OFF_STA_IS_ASSOC: usize = 100; // __le32 (0 = not yet associated)`

```
// __le32 (0 = not yet associated)
```

## L447 · `pub const MC_OFF_STA_DTIM_TIME: usize = 104; // __le32 (device time of next DTIM)`

```
// __le32 (device time of next DTIM)
```

## L448 · `pub const MC_OFF_STA_DTIM_TSF: usize = 108;  // __le64 (AP TSF of next DTIM)`

```
// __le64 (AP TSF of next DTIM)
```

## L449 · `pub const MC_OFF_STA_BI: usize = 116;        // __le32 beacon interval (TU)`

```
// __le32 beacon interval (TU)
```

## L450 · `pub const MC_OFF_STA_DTIM_INTERVAL: usize = 124; // __le32 (bi * dtim_period)`

```
// __le32 (bi * dtim_period)
```

## L451 · `pub const MC_OFF_STA_LISTEN_INTERVAL: usize = 132; // __le32`

```
// __le32
```

## L452 · `pub const MC_OFF_STA_ASSOC_ID: usize = 136;  // __le32 (aid; 0 before assoc)`

```
// __le32 (aid; 0 before assoc)
```

## L453 · `pub const MC_OFF_STA_BEACON_ARRIVE: usize = 140; // __le32 assoc_beacon_arrive_time`

```
// __le32 assoc_beacon_arrive_time
```

## L454 · `pub const MAC_FILTER_ACCEPT_GRP: u32 = 1 << 2;`

```
// filter flags (enum iwl_mac_filter_flags): accept multicast + foreign beacons.
```

## L457-458 · `pub const MAC_CCK_RATES_DEFAULT: u32 = 0x0F;`

```
// Default basic-rate ACK bitmaps for an empty BSSBasicRateSet (iwl_mvm_ack_rates
// with basic_rates == 0): mandatory CCK 1/2/5.5/11 = 0x0F, OFDM 6/12/24 = 0x15.
```

## L462-470 · `pub const MCC_UPDATE_CMD: u8 = 0xc8;`

```
// ── Stage 4d2a': regulatory domain (MCC_UPDATE_CMD, fw/api/nvm-reg.h) ──
// With LAR enabled (our NVM reported lar=1) the firmware refuses to scan until
// the regulatory domain is set: "Disallow scans that might crash the FW while
// the LAR regdomain is not set" (mvm/nvm.c iwl_mvm_init_mcc). iwl_mvm_up calls
// init_mcc right before config_scan. The initial update queries the FW's own
// default with alpha2 "ZZ" and source GET_CURRENT (mvm/mac80211.c
// iwl_mvm_get_current_regdomain → iwl_mvm_update_mcc); the FW replies with its
// chosen MCC + channel profile, after which scans are allowed. MCC_UPDATE_CMD
// is opcode 0xc8 in the legacy group and carries CMD_WANT_SKB (it responds).
```

## L472-473 · `pub const MCC_UPDATE_CMD_LEN: usize = 28;`

```
// struct iwl_mcc_update_cmd: __le16 mcc, u8 source_id, u8 reserved, __le32 key,
// u8 reserved2[20] = 28 bytes.
```

## L475 · `pub const MCC_OFF_MCC: usize = 0; // __le16 (alpha2[0] << 8 | alpha2[1])`

```
// __le16 (alpha2[0] << 8 | alpha2[1])
```

## L476 · `pub const MCC_OFF_SOURCE: usize = 2; // u8 source_id`

```
// u8 source_id
```

## L477 · `pub const MCC_ALPHA2_ZZ: u16 = ((b'Z' as u16) << 8) | (b'Z' as u16); // 0x5A5A`

```
// 0x5A5A
```

## L478 · `pub const MCC_SOURCE_GET_CURRENT: u8 = 0x10; // enum iwl_mcc_source`

```
// enum iwl_mcc_source
```

## L479-480 · `pub const MCC_RESP_OFF_STATUS: usize = 0;`

```
// iwl_mcc_update_resp_v8 payload offsets (relative to the rx packet data[]):
// __le32 status, __le16 mcc, ... __le32 n_channels @ 20.
```

## L485-492 · `pub const DATA_PATH_GROUP: u8 = 0x5; // fw/api/commands.h (SYSTEM_GROUP = 0x2 above)`

```
// ── Stage 4d2b1d: the rest of the mandatory iwl_mvm_up pre-scan cmds ──
// The full sequence iwl_mvm_up runs between ALIVE and config_scan, faithfully:
// TX_ANT → BT coex → SOC latency → DQA → device power → MCC → SCAN_CFG.
// configure_rxq + rss_cfg are no-ops for num_rxqs==1 (both return early); the
// BIOS/ACPI-gated cmds (lari/ppag/sar/sgom/tas) send nothing without platform
// tables (e.g. ppag !approved → return 0, sgom !enabled → return 0), exactly
// as Linux on a machine without them; the post-config_scan tuning isn't a scan
// prerequisite. The cap-gated ones below check the firmware capability bitmap.
```

## L493 · `pub const DATA_PATH_GROUP: u8 = 0x5; // fw/api/commands.h (SYSTEM_GROUP = 0x2 above)`

```
// fw/api/commands.h (SYSTEM_GROUP = 0x2 above)
```

## L495-496 · `pub const BT_CONFIG: u8 = 0x9b;`

```
// iwl_mvm_send_bt_init_conf (mvm/coex.c): BT_CONFIG, legacy group, struct
// iwl_bt_coex_cmd { __le32 mode; __le32 enabled_modules } = 8 bytes.
```

## L499 · `pub const BT_COEX_NW: u32 = 0x1; // enum: mode = network-coexistence`

```
// enum: mode = network-coexistence
```

## L500 · `pub const BT_COEX_SYNC2SCO_ENABLED: u32 = 1 << 2; // IWL_MVM_BT_COEX_SYNC2SCO=1 → always`

```
// IWL_MVM_BT_COEX_SYNC2SCO=1 → always
```

## L501 · `pub const BT_COEX_MPLUT_ENABLED: u32 = 1 << 0; // only if BT_MPLUT_SUPPORT cap`

```
// only if BT_MPLUT_SUPPORT cap
```

## L502 · `pub const BT_COEX_HIGH_BAND_RET: u32 = 1 << 4; // always`

```
// always
```

## L504-507 · `pub const SOC_CONFIGURATION_CMD: u8 = 0x01;`

```
// iwl_set_soc_latency (fw/init.c): SOC_CONFIGURATION_CMD in SYSTEM_GROUP, struct
// iwl_soc_configuration_cmd { __le32 flags; __le32 latency } = 8 bytes. AX200 is
// a discrete card (mac_cfg.integrated unset) → flags = DISCRETE, latency = 0.
// Gated by the SOC_LATENCY_SUPPORT capability.
```

## L512-514 · `pub const DQA_ENABLE_CMD: u8 = 0x0;`

```
// iwl_mvm_send_dqa_cmd (mvm/fw.c): DQA_ENABLE_CMD in DATA_PATH_GROUP, struct
// iwl_dqa_enable_cmd { __le32 cmd_queue } = 4 bytes. cmd_queue = the command
// queue id (IWL_MVM_DQA_CMD_QUEUE = 0 = IWL_CMD_QUEUE_ID). Gated by DQA_SUPPORT.
```

## L518-520 · `pub const POWER_TABLE_CMD: u8 = 0x77;`

```
// iwl_mvm_power_update_device (mvm/power.c): POWER_TABLE_CMD, legacy group,
// struct iwl_device_power_cmd { __le16 flags; __le16 reserved } = 4 bytes. The
// default power scheme is BPS (not CAM) so power-save is enabled.
```

## L525-528 · `pub const IWL_UCODE_TLV_ENABLED_CAPABILITIES: u32 = 30;`

```
// FW capability bitmap (IWL_UCODE_TLV_ENABLED_CAPABILITIES = 30, fw/file.h).
// Each such TLV is struct iwl_ucode_capa { __le32 api_index; __le32 api_capa };
// capability bit N is set iff a TLV has api_index == N/32 and bit (N%32) in
// api_capa (iwl-drv.c iwl_set_ucode_capabilities). Bit numbers from fw/file.h.
```

## L534-537 · `pub const HBUS_TARG_MEM_RADDR: u32 = 0x40C; // HBUS_BASE(0x400)+0x0C`

```
// ── FW error-log dump (iwl_mvm_dump_nic_error_log) ───────────────
// Read the firmware's lmac/umac error tables from device SRAM via the HBUS
// periphery-memory window. If valid != 0 the firmware asserted; error_id +
// the last hcmd/cmd_header reveal which command faulted it.
```

## L538 · `pub const HBUS_TARG_MEM_RADDR: u32 = 0x40C; // HBUS_BASE(0x400)+0x0C`

```
// HBUS_BASE(0x400)+0x0C
```

## L539 · `pub const HBUS_TARG_MEM_RDAT: u32 = 0x41C; // auto-incrementing read data`

```
// auto-incrementing read data
```

## L540 · `pub const CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ: u32 = 0x0000_0008;`

```
// CSR_GP_CNTRL: grab NIC access to read SRAM (iwl_pcie_grab_nic_access).
```

## L543 · `pub const LERR_VALID: usize = 0;`

```
// iwl_error_event_table (lmac) / iwl_umac_error_event_table u32 field indices.
```

## L551 · `pub const LERR_WORDS: usize = 32; // enough to reach last_cmd_id`

```
// enough to reach last_cmd_id
```

## L558-561 · `pub const REPLY_RX_MPDU_CMD: u8 = 0xc1;`

```
// ── Stage 4d2b2: beacon / RX MPDU parse (fw/api/rx.h, mvm/rxmq.c) ──
// REPLY_RX_MPDU_CMD (0xc1, LEGACY_GROUP) carries an iwl_rx_mpdu_desc followed by
// the 802.11 frame. For family < AX210 the descriptor is IWL_RX_DESC_SIZE_V1 =
// offsetofend(struct iwl_rx_mpdu_desc, v1) = 20 (common head) + 28 (v1) = 48.
```

## L564 · `pub const MPDU_OFF_MPDU_LEN: usize = 0; // __le16`

```
// Field offsets within iwl_rx_mpdu_desc, relative to pkt->data (RX_PKT_DATA_OFF).
```

## L565 · `pub const MPDU_OFF_MPDU_LEN: usize = 0; // __le16`

```
// __le16
```

## L566 · `pub const MPDU_OFF_MAC_FLAGS1: usize = 2; // u8`

```
// u8
```

## L567 · `pub const MPDU_OFF_MAC_FLAGS2: usize = 3; // u8`

```
// u8
```

## L568 · `pub const MPDU_OFF_AMSDU_INFO: usize = 4; // u8`

```
// u8
```

## L569 · `pub const MPDU_OFF_PHY_INFO: usize = 5; // __le16 (enum iwl_rx_mpdu_phy_info)`

```
// __le16 (enum iwl_rx_mpdu_phy_info)
```

## L570-571 · `pub const IWL_RX_MPDU_PHY_AMPDU: u16 = 1 << 5;`

```
// fw/api/rx.h: the firmware flips TOGGLE at the start of every new aggregate,
// which is how iwl_mvm_rx_mpdu_mq tells one A-MPDU from the next.
```

## L574 · `pub const MPDU_OFF_STATUS: usize = 12; // __le32 (enum iwl_rx_mpdu_status)`

```
// __le32 (enum iwl_rx_mpdu_status)
```

## L575 · `pub const MPDU_OFF_REORDER_DATA: usize = 16; // __le32`

```
// __le32
```

## L576 · `pub const MPDU_OFF_RATE_N_FLAGS: usize = 28; // v1.rate_n_flags (union @20 + 8)`

```
// v1.rate_n_flags (union @20 + 8)
```

## L577 · `pub const MPDU_OFF_ENERGY_A: usize = 32; // v1.energy_a (union @20 + v1 offset 12)`

```
// v1.energy_a (union @20 + v1 offset 12)
```

## L578 · `pub const MPDU_OFF_ENERGY_B: usize = 33; // v1.energy_b`

```
// v1.energy_b
```

## L579 · `pub const MPDU_OFF_CHANNEL: usize = 34; // v1.channel`

```
// v1.channel
```

## L580 · `pub const MPDU_OFF_GP2_ON_AIR: usize = 36; // v1.gp2_on_air_rise __le32`

```
// v1.gp2_on_air_rise __le32
```

## L581 · `pub const MPDU_OFF_TSF_ON_AIR: usize = 40; // v1.tsf_on_air_rise __le64`

```
// v1.tsf_on_air_rise __le64
```

## L582-585 · `pub const IWL_RX_REORDER_DATA_INVALID_BAID: u8 = 0x7f;`

```
// reorder_data (fw/api/rx.h): what the firmware knows about this MPDU's place in
// its block-ack window. BAID 0x7f means "no session" — everything else is the
// session id the firmware handed back when we started it. NSSN is the sequence
// number the firmware considers the first unreceived one; SN is this frame's.
```

## L588-596 · `pub const RX_BAID_ALLOCATION_CONFIG_CMD: u8 = 0x16;`

```
// ── RX block-ack allocation — RX_BAID_ALLOCATION_CONFIG_CMD (DATA_PATH/0x16) ──
//
// iwl_mvm_fw_baid_op (mvm/sta.c) picks between TWO ways to open an RX
// aggregation session, and we had ported only one of them. Firmware that
// advertises IWL_UCODE_TLV_CAPA_BAID_ML_SUPPORT wants THIS command; the
// ADD_STA route with STA_MODIFY_ADD_BA_TID is the fallback for older
// firmware. Sending the fallback to a card that wants this one is not merely
// ignored — the firmware answers nothing and stops completing transmissions,
// which costs the whole link.
```

## L600 · `pub const IWL_RX_BAID_ACTION_ADD: u32 = 0;`

```
// enum iwl_rx_baid_action
```

## L604-606 · `pub const BAID_CFG_CMD_LEN: usize = 16;`

```
// struct iwl_rx_baid_cfg_cmd: __le32 action, then a union. The `alloc` arm is
// __le32 sta_id_mask, u8 tid, u8 reserved[3], __le16 ssn, __le16 win_size;
// the `remove` arm is __le32 sta_id_mask, __le32 tid. 16 bytes either way.
```

## L609 · `pub const BAID_OFF_STA_MASK: usize = 4;   // shared by alloc and remove`

```
// shared by alloc and remove
```

## L610 · `pub const BAID_OFF_ALLOC_TID: usize = 8;  // u8`

```
// u8
```

## L611 · `pub const BAID_OFF_ALLOC_SSN: usize = 12; // __le16`

```
// __le16
```

## L612 · `pub const BAID_OFF_ALLOC_WIN: usize = 14; // __le16`

```
// __le16
```

## L613 · `pub const BAID_OFF_REMOVE_TID: usize = 8; // __le32`

```
// __le32
```

## L615-617 · `pub const IWL_MAX_BAID: u32 = 32;`

```
// struct iwl_rx_baid_cfg_resp is a bare __le32 baid. Linux rejects anything
// outside the map (IWL_MAX_BAID); a firmware error arrives as a negative,
// which reaches us as a very large unsigned value.
```

## L628-633 · `pub const MISSED_BEACONS_NOTIFICATION: u8 = 0xa2;`

```
// ── Missed beacons (fw/api/mac.h, mvm/mac-ctxt.c) ─────────────────────────
// Once associated the firmware STOPS passing beacons to the host — Linux sets
// MAC_FILTER_IN_BEACON only while unassociated (mac-ctxt.c:704-711). Noticing
// that the AP is gone is therefore not our job but the firmware's, and this
// notification is how it tells us. Ignoring it means sitting on a dead link:
// exactly what an AP does when it steers a client to the other mesh node.
```

## L635 · `pub const MB_OFF_SINCE_LAST_RX: usize = 4;  // __le32 consec_missed_beacons_since_last_rx`

```
// __le32 consec_missed_beacons_since_last_rx
```

## L636 · `pub const MB_OFF_CONSEC: usize = 8;         // __le32 consec_missed_beacons`

```
// __le32 consec_missed_beacons
```

## L637 · `pub const MB_OFF_EXPECTED: usize = 12;      // __le32 num_expected_beacons`

```
// __le32 num_expected_beacons
```

## L638 · `pub const MB_OFF_RECEIVED: usize = 16;      // __le32 num_recvd_beacons`

```
// __le32 num_recvd_beacons
```

## L639-640 · `pub const IWL_MVM_MISSED_BEACONS_SINCE_RX_THOLD: u32 = 4;`

```
// iwl-modparams.h. Long threshold + "nothing received since" = the link is gone;
// many missed beacons WHILE data still flows is not, and Linux says so out loud.
```

## L645-646 · `pub const FRAME_RELEASE: u8 = 0xc3;`

```
// The firmware tells us the window may move even when no frame arrives (it saw
// the frames on air but had nothing to deliver): baid, reserved, __le16 nssn.
```

## L650-651 · `pub const MFLG1_MIC_CRC_LEN_MASK: u8 = 0xf0;`

```
// mac_flags1: bits 7:4 = (MIC+CRC length / 2) the RADA may not have stripped.
// iwl_mvm_create_skb: mic_crc_len = u8_get_bits(mac_flags1, 0xf0) << 1.
```

## L653-655 · `pub const MFLG2_PAD: u8 = 0x20;`

```
// mac_flags2: the firmware DWORD-aligns the payload by inserting 2 bytes AFTER
// the IV when (802.11 header + IV) is not a multiple of 4 — which is exactly the
// QoS-header + CCMP case (26 + 8 = 34). Missing this shifts every payload by 2.
```

## L657 · `pub const RX_STATUS_SEC_MASK: u32 = 0x7 << 8;`

```
// iwl_rx_mpdu_status: which cipher the firmware decrypted with (→ IV length).
```

## L661-663 · `pub const RX_STATUS_MIC_OK: u32 = 1 << 6;`

```
// …and whether it worked. For CCM/GCM iwl_mvm_rx_crypto checks exactly one bit
// and drops the frame when it is clear (rxmq.c:452). We only count — a
// diagnostic that changes the data path cannot measure the data path.
```

## L667 · `pub const DOT11_HDR_LEN: usize = 24; // fc(2)+dur(2)+addr1/2/3(18)+seq(2)`

```
// 802.11 management frame (ieee80211_hdr + beacon/probe-response body).
```

## L668 · `pub const DOT11_HDR_LEN: usize = 24; // fc(2)+dur(2)+addr1/2/3(18)+seq(2)`

```
// fc(2)+dur(2)+addr1/2/3(18)+seq(2)
```

## L669 · `pub const DOT11_OFF_ADDR1: usize = 4;  // DA (receiver)`

```
// DA (receiver)
```

## L670 · `pub const DOT11_OFF_ADDR2: usize = 10; // SA (transmitter)`

```
// SA (transmitter)
```

## L671 · `pub const DOT11_OFF_ADDR3: usize = 16; // BSSID`

```
// BSSID
```

## L672 · `pub const DOT11_OFF_SEQ: usize = 22;   // seq_ctrl __le16 (frag 0-3 | seq_num 4-15)`

```
// seq_ctrl __le16 (frag 0-3 | seq_num 4-15)
```

## L673 · `pub const DOT11_BEACON_FIXED: usize = 12; // timestamp(8)+beacon_int(2)+capab(2)`

```
// timestamp(8)+beacon_int(2)+capab(2)
```

## L674 · `pub const DOT11_OFF_IES: usize = DOT11_HDR_LEN + DOT11_BEACON_FIXED; // 36`

```
// 36
```

## L676 · `pub const DOT11_STYPE_BEACON: u8 = 8;`

```
// Management-frame subtypes (frame_control bits 4-7) we collect APs from.
```

## L679 · `pub const MAX_APS: usize = 64; // both bands → more networks than 2.4 GHz alone`

```
// both bands → more networks than 2.4 GHz alone
```

## L682-684 · `pub const PHY_BAND_5_U8: u8 = 0; // PHY_BAND_5`

```
// ── Stage 5a: connect — PHY context + RLC + binding ──────────────
// All cmd_vers parsed from the FW file: PHY_CONTEXT v4, RLC_CONFIG v2,
// BINDING v2 (BINDING_CDB_SUPPORT=yes → full struct), CDB_SUPPORT=no → lmac_id 0.
```

## L685 · `pub const PHY_BAND_5_U8: u8 = 0; // PHY_BAND_5`

```
// PHY_BAND_5
```

## L687-688 · `pub const IWL_PHY_CHANNEL_MODE40: u8 = 1;`

```
/// fw/api/phy-ctxt.h:17. MODE80 = 0x2 and MODE160 = 0x3 exist too — the next
/// rung, once VHT is negotiated.
```

## L691-692 · `pub const IWL_PHY_CTRL_POS_OFFS_MSK: u8 = 0x3;`

```
/// phy-ctxt.h:37 — for VHT, bits 1:0 are the control channel's distance from
/// the centre in 20 MHz steps; bit 2 says it sits above.
```

## L694-695 · `pub const IWL_PHY_CTRL_POS_ABOVE: u8 = 0x4;`

```
/// Control-channel position (phy-ctxt.h:35). For HT the bit simply means "the
/// control channel is the UPPER of the two", i.e. the secondary sits below.
```

## L697 · `pub const IWL_LMAC_24G_INDEX: u32 = 0; // no CDB → lmac_id always 0`

```
// no CDB → lmac_id always 0
```

## L700-701 · `pub const PHY_CTX_CMD_LEN: usize = 32;`

```
// PHY_CONTEXT_CMD v4 (fw/api/phy-ctxt.h, struct iwl_phy_context_cmd, 32 B).
// PHY_CONTEXT_CMD = 0x08 already defined above (scan section).
```

## L703 · `pub const PC_OFF_ID_COLOR: usize = 0;     // __le32 FW_CMD_ID_AND_COLOR(phy_id=0,0)=0`

```
// __le32 FW_CMD_ID_AND_COLOR(phy_id=0,0)=0
```

## L704 · `pub const PC_OFF_ACTION: usize = 4;       // __le32 FW_CTXT_ACTION_ADD`

```
// __le32 FW_CTXT_ACTION_ADD
```

## L705 · `pub const PC_OFF_CI_CHANNEL: usize = 8;   // ci.channel __le32`

```
// ci.channel __le32
```

## L706 · `pub const PC_OFF_CI_BAND: usize = 12;     // ci.band u8 (PHY_BAND_24/5)`

```
// ci.band u8 (PHY_BAND_24/5)
```

## L707 · `pub const PC_OFF_CI_WIDTH: usize = 13;    // ci.width u8 (MODE20)`

```
// ci.width u8 (MODE20)
```

## L708 · `pub const PC_OFF_CI_CTRL_POS: usize = 14; // ci.ctrl_pos u8 (0 for 20 MHz)`

```
// ci.ctrl_pos u8 (0 for 20 MHz)
```

## L709 · `pub const PC_OFF_LMAC_ID: usize = 16;     // __le32 (ci.reserved @15)`

```
// __le32 (ci.reserved @15)
```

## L710-714 · `pub const PC_OFF_RXCHAIN: usize = 20;     // __le32 rxchain_info`

```
// struct iwl_phy_context_cmd continues: rxchain_info @20, dsp_cfg_flags @24,
// secondary_ctrl_chnl_loc @28. rxchain_info is the SAME encoding the RLC
// command uses (iwl_mvm_phy_ctxt_set_rxchain fills both from one helper), and
// leaving it zero means PHY_RX_CHAIN_VALID = 0 — no receive antennas declared
// for this PHY context at all.
```

## L715 · `pub const PC_OFF_RXCHAIN: usize = 20;     // __le32 rxchain_info`

```
// __le32 rxchain_info
```

## L717-718 · `pub const RLC_CONFIG_CMD: u8 = 0x08;`

```
// rxchain_info @20 (reserved in v4 → 0), dsp_cfg_flags @24,
// secondary_ctrl_chnl_loc @28, reserved[3] @29 — all 0.
```

## L720-721 · `pub const RLC_CONFIG_CMD: u8 = 0x08;`

```
// RLC_CONFIG_CMD v2 (DATA_PATH_GROUP/0x08, struct iwl_rlc_config_cmd, 32 B).
// Required: RLC cmd_ver=2 (< 3 = not offloaded), so the driver sends rx_chain_info.
```

## L724 · `pub const RLC_OFF_PHY_ID: usize = 0;        // __le32 phy_id`

```
// __le32 phy_id
```

## L725 · `pub const RLC_OFF_RX_CHAIN_INFO: usize = 4; // rlc.rx_chain_info __le32`

```
// rlc.rx_chain_info __le32
```

## L726-728 · `pub const RLC_RX_CHAIN_INFO_2X2: u32 = (0x3 << 1) | (2 << 10) | (2 << 12); // 0x2806`

```
// rlc.reserved @8; sad{chain_a@12,chain_b@16,mac_id@20,reserved@24}=0; flags@28; rsv@29.
// iwl_mvm_phy_ctxt_set_rxchain: valid_rx_ant<<PHY_RX_CHAIN_VALID_POS(1) |
// idle_cnt<<CNT_POS(10) | active_cnt<<MIMO_CNT_POS(12). 2x2 + diversity → idle=active=2.
```

## L729 · `pub const RLC_RX_CHAIN_INFO_2X2: u32 = (0x3 << 1) | (2 << 10) | (2 << 12); // 0x2806`

```
// 0x2806
```

## L731 · `pub const BINDING_CONTEXT_CMD: u8 = 0x2b;`

```
// BINDING_CONTEXT_CMD v2 (0x2b, struct iwl_binding_cmd, 28 B; CDB → full w/ lmac_id).
```

## L734 · `pub const BC_OFF_ID_COLOR: usize = 0; // FW_CMD_ID_AND_COLOR(phy_id=0,0)=0`

```
// FW_CMD_ID_AND_COLOR(phy_id=0,0)=0
```

## L735 · `pub const BC_OFF_ACTION: usize = 4;   // FW_CTXT_ACTION_ADD`

```
// FW_CTXT_ACTION_ADD
```

## L736 · `pub const BC_OFF_MACS: usize = 8;     // __le32 macs[3] (MAX_MACS_IN_BINDING)`

```
// __le32 macs[3] (MAX_MACS_IN_BINDING)
```

## L737 · `pub const BC_OFF_PHY: usize = 20;     // __le32 phy = FW_CMD_ID_AND_COLOR(phy_id=0,0)`

```
// __le32 phy = FW_CMD_ID_AND_COLOR(phy_id=0,0)
```

## L738 · `pub const BC_OFF_LMAC_ID: usize = 24; // __le32 lmac_id`

```
// __le32 lmac_id
```

## L740-742 · `pub const ADD_STA_CMD_LEN: usize = 48;`

```
// ── Stage 5b: connect — station (AP peer) + gen2 TX queue ─────────
// ADD_STA v12 (fw/api/sta.h, struct iwl_mvm_add_sta_cmd, 48 B). ADD_STA = 0x18
// already defined above. Status response (ADD_STA_SUCCESS = 0x1).
```

## L744 · `pub const AS_OFF_ADD_MODIFY: usize = 0;   // u8 (0 = add)`

```
// u8 (0 = add)
```

## L745 · `pub const AS_OFF_TID_DISABLE: usize = 2;  // __le16 tid_disable_tx`

```
// __le16 tid_disable_tx
```

## L746 · `pub const AS_OFF_MAC_ID_COLOR: usize = 4; // __le32 FW_CMD_ID_AND_COLOR(0,0)=0`

```
// __le32 FW_CMD_ID_AND_COLOR(0,0)=0
```

## L747 · `pub const AS_OFF_ADDR: usize = 8;         // u8[6] AP BSSID`

```
// u8[6] AP BSSID
```

## L748 · `pub const AS_OFF_STA_ID: usize = 16;      // u8`

```
// u8
```

## L749 · `pub const AS_OFF_STATION_FLAGS: usize = 20;     // __le32`

```
// __le32
```

## L750 · `pub const AS_OFF_STATION_FLAGS_MSK: usize = 24; // __le32`

```
// __le32
```

## L751 · `pub const AS_OFF_STATION_TYPE: usize = 35;      // u8`

```
// u8
```

## L752 · `pub const AS_OFF_ASSOC_ID: usize = 36;          // __le16 (set from IEEE80211_STA_ASSOC on)`

```
// __le16 (set from IEEE80211_STA_ASSOC on)
```

## L753 · `pub const IWL_STA_LINK: u8 = 0;           // enum iwl_sta_type`

```
// enum iwl_sta_type
```

## L755 · `pub const ADD_STA_STATUS_MASK: u32 = 0xff; // IWL_ADD_STA_STATUS_MASK`

```
// IWL_ADD_STA_STATUS_MASK
```

## L756-759 · `pub const AS_OFF_MODIFY_MASK: usize = 17;       // u8`

```
// The immediate-block-ack fields of the same command (iwl_mvm_fw_baid_op_sta).
// A modify carrying STA_MODIFY_ADD_BA_TID starts the RX aggregation session in
// the firmware; the response's status word carries the session id (BAID) that
// then appears in every aggregated frame's reorder_data.
```

## L760 · `pub const AS_OFF_MODIFY_MASK: usize = 17;       // u8`

```
// u8
```

## L761 · `pub const AS_OFF_ADD_IMM_BA_TID: usize = 28;    // u8`

```
// u8
```

## L762 · `pub const AS_OFF_REMOVE_IMM_BA_TID: usize = 29; // u8`

```
// u8
```

## L763 · `pub const AS_OFF_ADD_IMM_BA_SSN: usize = 30;    // __le16`

```
// __le16
```

## L764 · `pub const AS_OFF_RX_BA_WINDOW: usize = 44;      // __le16`

```
// __le16
```

## L771 · `pub const TID_DISABLE_AGG_INIT: u16 = 0xffff; // "No aggs at first" (sta.c:1779)`

```
// "No aggs at first" (sta.c:1779)
```

## L772 · `pub const STA_FLAGS_MSK_ADD: u32 = (3 << 26) | (3 << 28) | (1 << 17); // 0x3C020000`

```
// station_flags_msk for the add: FAT_EN(3<<26) | MIMO_EN(3<<28) | RTS_MIMO_PROT(BIT17).
```

## L773 · `pub const STA_FLAGS_MSK_ADD: u32 = (3 << 26) | (3 << 28) | (1 << 17); // 0x3C020000`

```
// 0x3C020000
```

## L774-777 · `pub const STA_FLG_FAT_EN_20MHZ: u32 = 0 << 26;`

```
// enum iwl_sta_flags (fw/api/sta.h). Applied at assoc once the peer's HT caps
// are known (iwl_mvm_sta_send_to_fw): channel width, spatial streams, and the
// A-MPDU limits the AP advertised. Without them the firmware keeps the station
// at its "just added" defaults and TLC has nothing to scale into.
```

## L779-781 · `pub const STA_FLG_FAT_EN_40MHZ: u32 = 1 << 26;`

```
/// fw/api/sta.h:87 — the station's TX channel width. A two-bit FIELD, so the
/// value replaces rather than ORs; 20 MHz being 0 is why leaving it unset
/// silently pins transmission to 20 MHz however wide the PHY is configured.
```

## L791 · `pub const AP_STA_ID: u8 = 0;              // first free station table index`

```
// first free station table index
```

## L793-794 · `pub const SCD_QUEUE_CONFIG_CMD: u8 = 0x17;`

```
// SCD_QUEUE_CONFIG_CMD v3 (DATA_PATH_GROUP/0x17, struct iwl_scd_queue_cfg_cmd
// ADD union, 36 B). gen2 dynamic TX queue allocation.
```

## L798 · `pub const SQ_OFF_OPERATION: usize = 0;   // __le32`

```
// __le32
```

## L799 · `pub const SQ_OFF_STA_MASK: usize = 4;    // __le32 BIT(sta_id)`

```
// __le32 BIT(sta_id)
```

## L800 · `pub const SQ_OFF_TID: usize = 8;         // u8`

```
// u8
```

## L801 · `pub const SQ_OFF_FLAGS: usize = 12;      // __le32 (0 for v3 ADD)`

```
// __le32 (0 for v3 ADD)
```

## L802 · `pub const SQ_OFF_CB_SIZE: usize = 16;    // __le32 TFD_QUEUE_CB_SIZE(size)=ilog2(size)-3`

```
// __le32 TFD_QUEUE_CB_SIZE(size)=ilog2(size)-3
```

## L803 · `pub const SQ_OFF_BC_DRAM_ADDR: usize = 20; // __le64`

```
// __le64
```

## L804 · `pub const SQ_OFF_TFDQ_DRAM_ADDR: usize = 28; // __le64`

```
// __le64
```

## L805 · `pub const SQ_RSP_OFF_QUEUE_NUMBER: usize = 0;`

```
// iwl_tx_queue_cfg_rsp: queue_number __le16 @0, flags @2, write_pointer @4.
```

## L808-809 · `pub const IWL_MGMT_TID: u8 = 15;`

```
// MGMT queue (auth/assoc TX): tid = IWL_MGMT_TID, size = IWL_MGMT_QUEUE_SIZE
// (22000 has no min_txq_size → max(16,0)=16). cb_size = ilog2(16)-3 = 1.
```

## L813 · `pub const TFD_QUEUE_BC_SIZE: usize = 256 + 64;`

```
// bc table (gen2, non-AX210): iwl_bc_tbl_entry(__le16) * TFD_QUEUE_BC_SIZE(256+64).
```

## L815 · `pub const BC_TBL_BYTES: usize = TFD_QUEUE_BC_SIZE * 2; // 640`

```
// 640
```

## L817-822 · `pub const TLC_MNG_CONFIG_CMD: u8 = 0x0F;`

```
// ── Rate scaling (TLC offload) — TLC_MNG_CONFIG_CMD (DATA_PATH_GROUP/0x0F) ──
// struct iwl_tlc_config_cmd_v4 (fw/api/rs.h, TLC_MNG_CONFIG_CMD_API_S_VER_4),
// 28 bytes. Our FW reports cmd_ver=4. Without this command the firmware never
// rate-scales and every data frame goes out at the fixed host rate (1 Mbit CCK,
// tx_raw); with it the firmware picks the best per-frame rate from the station's
// advertised rate set (iwl_mvm_rs_fw_rate_init, mvm/rs-fw.c).
```

## L825 · `pub const TLC_OFF_STA_ID: usize = 0;       // u8`

```
// u8
```

## L826 · `pub const TLC_OFF_MAX_CH_WIDTH: usize = 4; // u8 (enum iwl_tlc_mng_cfg_cw)`

```
// u8 (enum iwl_tlc_mng_cfg_cw)
```

## L827 · `pub const TLC_OFF_MODE: usize = 5;         // u8 (enum iwl_tlc_mng_cfg_mode)`

```
// u8 (enum iwl_tlc_mng_cfg_mode)
```

## L828 · `pub const TLC_OFF_CHAINS: usize = 6;       // u8 (chain A|B mask, = ANT_AB)`

```
// u8 (chain A|B mask, = ANT_AB)
```

## L829 · `pub const TLC_OFF_SGI: usize = 7;          // u8 sgi_ch_width_supp`

```
// u8 sgi_ch_width_supp
```

## L830 · `pub const TLC_OFF_FLAGS: usize = 8;        // __le16`

```
// __le16
```

## L831 · `pub const TLC_OFF_NON_HT_RATES: usize = 10; // __le16`

```
// __le16
```

## L832 · `pub const TLC_OFF_HT_RATES: usize = 12;    // __le16[2][3] (12 B, 0 for legacy)`

```
// __le16[2][3] (12 B, 0 for legacy)
```

## L833 · `pub const TLC_OFF_MAX_MPDU: usize = 24;    // __le16`

```
// __le16
```

## L834 · `pub const TLC_OFF_MAX_TXOP: usize = 26;    // __le16`

```
// __le16
```

## L835 · `pub const TLC_CH_WIDTH_20MHZ: u8 = 0;      // IWL_TLC_MNG_CH_WIDTH_20MHZ`

```
// IWL_TLC_MNG_CH_WIDTH_20MHZ
```

## L836 · `pub const TLC_CH_WIDTH_40MHZ: u8 = 1;      // IWL_TLC_MNG_CH_WIDTH_40MHZ`

```
// IWL_TLC_MNG_CH_WIDTH_40MHZ
```

## L837 · `pub const TLC_CH_WIDTH_80MHZ: u8 = 2;      // IWL_TLC_MNG_CH_WIDTH_80MHZ`

```
// IWL_TLC_MNG_CH_WIDTH_80MHZ
```

## L838 · `pub const TLC_MODE_NON_HT: u8 = 0;         // IWL_TLC_MNG_MODE_NON_HT`

```
// IWL_TLC_MNG_MODE_NON_HT
```

## L839 · `pub const TLC_MODE_HT: u8 = 1;             // IWL_TLC_MNG_MODE_HT`

```
// IWL_TLC_MNG_MODE_HT
```

## L840 · `pub const TLC_MODE_VHT: u8 = 2;            // IWL_TLC_MNG_MODE_VHT`

```
// IWL_TLC_MNG_MODE_VHT
```

## L841-842 · `pub const TLC_OFF_HT_RATES_NSS1: usize = TLC_OFF_HT_RATES;     // [0][0]`

```
// ht_rates is __le16[IWL_TLC_NSS_MAX=2][IWL_TLC_MCS_PER_BW_NUM_V4=3]; HT only
// ever fills the [nss][IWL_TLC_MCS_PER_BW_80=0] slot (rs_fw_set_supp_rates).
```

## L843 · `pub const TLC_OFF_HT_RATES_NSS1: usize = TLC_OFF_HT_RATES;     // [0][0]`

```
// [0][0]
```

## L844 · `pub const TLC_OFF_HT_RATES_NSS2: usize = TLC_OFF_HT_RATES + 6; // [1][0]`

```
// [1][0]
```

## L845 · `pub const TLC_FLAGS_STBC: u16 = 1 << 0;`

```
// enum iwl_tlc_mng_cfg_flags
```

## L848-851 · `pub const TLC_MNG_UPDATE_NOTIF: u8 = 0xF7;`

```
// TLC_MNG_UPDATE_NOTIF (DATA_PATH_GROUP/0xF7, notif_ver 3): the firmware reports
// the rate it settled on. This is the ONLY way to see the negotiated air rate
// from the host — without it we cannot tell 1 Mbit CCK from MCS 15.
// struct iwl_tlc_update_notif: sta_id u8, reserved[3], flags __le32, rate __le32.
```

## L853 · `pub const TLC_NOTIF_OFF_RATE: usize = 8;   // __le32 rate_n_flags`

```
// __le32 rate_n_flags
```

## L854 · `pub const RATE_MCS_MOD_TYPE_POS: u32 = 8;`

```
// rate_n_flags v2 field extraction (same format as the TX command, see below).
```

## L862 · `pub const RATE_MCS_CODE_MSK: u32 = 0x1f;   // MCS index / legacy rate index`

```
// MCS index / legacy rate index
```

## L863 · `pub const RATE_MCS_NSS_MSK: u32 = 0x20;    // 0 = 1 stream, 1 = 2 streams`

```
// 0 = 1 stream, 1 = 2 streams
```

## L864-868 · `pub const RATE_MCS_NSS_MSK_V2: u32 = 0x10;`

```
// …but only in the v3 format. This firmware reports v2 (TX_CMD cmd_ver 9 < 11),
// where the SAME field sits one bit lower. Everything else is identical —
// iwl_v3_rate_from_v2_v3 does nothing but move this one bit. Read with the v3
// mask, a v2 word turns "MCS 7, 2 streams" into "MCS 23, 1 stream": a rate that
// does not exist, and no Mbit figure at all.
```

## L870 · `pub const TX_CMD_VER_RATE_V3: u8 = 11;`

```
// fw_rates_ver: TX_CMD cmd_ver >= 11 means the firmware talks v3 (iwl_mvm_has_rate_v3).
```

## L874-876 · `pub const TLC_NON_HT_RATES_24: u16 = 0x0FFF;`

```
// Supported legacy-rate bitmap: BIT(hw_value) per rate (IWL_RATE_*_INDEX). Full
// 2.4 GHz 11g = CCK 1/2/5.5/11 (bits 0-3) + OFDM 6..54 (bits 4-11) = 0x0FFF;
// 5 GHz is OFDM-only (no CCK) = bits 4-11 = 0x0FF0.
```

## L880-883 · `pub const TX_CMD: u8 = 0x1c;`

```
// ── Stage 5c: gen2 TX data path (send an 802.11 frame) ───────────
// TX_CMD = 0x1c. The device TX command is a SHORT 4-byte iwl_cmd_header
// (cmd, group_id=0, sequence) followed by iwl_tx_cmd_v9 (TX_CMD cmd_ver=9) and
// the 802.11 frame. Unlike host commands, TX uses the short header (group 0).
```

## L885-887 · `pub const TXC_HDR_LEN: usize = 4;       // iwl_cmd_header (short)`

```
// iwl_tx_cmd_v9 (fw/api/tx.h): len/offload/flags/dram_info/rate_n_flags = 20 B,
// then the 802.11 header+body. Field offsets relative to the dev_cmd start
// (after the 4-byte cmd header), i.e. tx_cmd begins at dev_cmd offset 4.
```

## L888 · `pub const TXC_HDR_LEN: usize = 4;       // iwl_cmd_header (short)`

```
// iwl_cmd_header (short)
```

## L889 · `pub const TXC_OFF_LEN: usize = 4;       // tx_cmd_v9.len __le16 (full 802.11 frame len)`

```
// tx_cmd_v9.len __le16 (full 802.11 frame len)
```

## L890 · `pub const TXC_OFF_OFFLOAD: usize = 6;   // tx_cmd_v9.offload_assist __le16`

```
// tx_cmd_v9.offload_assist __le16
```

## L891 · `pub const TXC_OFF_FLAGS: usize = 8;     // tx_cmd_v9.flags __le32`

```
// tx_cmd_v9.flags __le32
```

## L892 · `pub const TXC_OFF_DRAM: usize = 12;     // tx_cmd_v9.dram_info (8 B, 0 = no key)`

```
// tx_cmd_v9.dram_info (8 B, 0 = no key)
```

## L893 · `pub const TXC_OFF_RATE: usize = 20;     // tx_cmd_v9.rate_n_flags __le32`

```
// tx_cmd_v9.rate_n_flags __le32
```

## L894 · `pub const TXC_OFF_FRAME: usize = 24;    // 802.11 frame (hdr[]) starts here`

```
// 802.11 frame (hdr[]) starts here
```

## L895 · `pub const IWL_TX_FLAGS_CMD_RATE: u32 = 1 << 0; // use rate_n_flags from the cmd`

```
// iwl_tx_flags (NEW gen2 set — NOT the gen1 TX_CMD_FLG_*).
```

## L896 · `pub const IWL_TX_FLAGS_CMD_RATE: u32 = 1 << 0; // use rate_n_flags from the cmd`

```
// use rate_n_flags from the cmd
```

## L897 · `pub const IWL_TX_FLAGS_ENCRYPT_DIS: u32 = 1 << 1; // unencrypted`

```
// unencrypted
```

## L898-902 · `pub const TX_CMD_OFFLD_MH_SIZE_POS: u16 = 8;`

```
// offload_assist (enum iwl_tx_offload_assist_flags_pos). iwl_mvm_tx_csum fills
// MH_SIZE for EVERY frame — the 802.11 header length in 2-byte words — and sets
// PAD when that length is not a multiple of 4, in which case the transport
// inserts 2 bytes between header and payload to DWORD-align the payload. That
// is exactly the QoS case (26 bytes); getting it wrong misplaces the CCMP IV.
```

## L905-907 · `pub const RATE_1M_CCK_ANT_A: u32 = 0x0000_4000;`

```
// rate_n_flags (fw_rates_ver=2: TX_CMD cmd_ver 9 ≥8 <11). Modern format:
// MOD_TYPE @ bit8 (CCK=0, LEGACY_OFDM=1<<8), legacy rate in bits 0-2, ANT @ bit14.
// 1 Mbps CCK ant A = 0x4000; 6 Mbps OFDM ant A = 0x4100 (iwl_v3_rate_to_v2_v3).
```

## L910 · `pub const DOT11_FC_AUTH: u8 = 0xB0; // frame_control byte0: type mgmt(0), subtype auth(11)`

```
// 802.11 management auth frame (open system): 24-byte header + 6-byte body.
```

## L911 · `pub const DOT11_FC_AUTH: u8 = 0xB0; // frame_control byte0: type mgmt(0), subtype auth(11)`

```
// frame_control byte0: type mgmt(0), subtype auth(11)
```

## L912 · `pub const DOT11_AUTH_BODY_LEN: usize = 6; // algorithm(2) + seq(2) + status(2)`

```
// algorithm(2) + seq(2) + status(2)
```

## L915 · `pub const DOT11_AUTH_SEQ_2: u16 = 2;  // AP's auth response`

```
// AP's auth response
```

## L918 · `pub const DOT11_FC_ASSOC_REQ: u8 = 0x00; // mgmt(0), subtype assoc-req(0)`

```
// ── Stage 5d: association request / response ──────────────────────
```

## L919 · `pub const DOT11_FC_ASSOC_REQ: u8 = 0x00; // mgmt(0), subtype assoc-req(0)`

```
// mgmt(0), subtype assoc-req(0)
```

## L920 · `pub const DOT11_STYPE_AUTH: u8 = 11;     // subtype of an auth frame`

```
// subtype of an auth frame
```

## L921 · `pub const DOT11_STYPE_ASSOC_RESP: u8 = 1; // subtype of an assoc response`

```
// subtype of an assoc response
```

## L922 · `pub const DOT11_STYPE_DISASSOC: u8 = 10; // mgmt subtype disassociation`

```
// mgmt subtype disassociation
```

## L923 · `pub const DOT11_STYPE_DEAUTH: u8 = 12;   // mgmt subtype deauthentication (mesh steering kick)`

```
// mgmt subtype deauthentication (mesh steering kick)
```

## L924-925 · `pub const ASSOC_RESP_OFF_STATUS: usize = 2; // within the 802.11 body`

```
// assoc-req fixed fields: capability(2) + listen_interval(2), then IEs.
// assoc-resp body: capability(2) + status_code(2) + aid(2), then IEs.
```

## L926 · `pub const ASSOC_RESP_OFF_STATUS: usize = 2; // within the 802.11 body`

```
// within the 802.11 body
```

## L929 · `pub const WLAN_CAP_ESS: u16 = 1 << 0;`

```
// ieee80211 capability bits (assoc-req).
```

## L934 · `pub const WLAN_EID_SUPP_RATES: u8 = 1;`

```
// information element ids.
```

## L938 · `pub const WLAN_EID_HT_OPERATION: u8 = 61;`

```
/// struct ieee80211_ht_operation: primary_chan(1), ht_param(1), ...
```

## L942 · `pub const IEEE80211_HT_PARAM_CHA_SEC_OFFSET: u8 = 0x03;`

```
/// ieee80211.h:2004 — secondary channel offset, bits 0-1 of ht_param.
```

## L947-948 · `pub const HT_OP_OFF_OPERATION_MODE: usize = 2; // __le16`

```
/// HT Operation `operation_mode`, ieee80211.h:2012. The AP states here which
/// protection the BSS needs; the firmware cannot know it any other way.
```

## L949 · `pub const HT_OP_OFF_OPERATION_MODE: usize = 2; // __le16`

```
// __le16
```

## L955 · `pub const WLAN_EID_ERP_INFO: u8 = 42;`

```
/// ERP information element (EID 42), ieee80211.h:3515.
```

## L958 · `pub const IEEE80211_HT_CAP_SUP_WIDTH_20_40: u16 = 0x0002;`

```
/// ieee80211.h:1914/1919
```

## L962-968 · `pub const WLAN_EID_EXTENSION: u8 = 255;`

```
// ── VHT (802.11ac) ────────────────────────────────────────────────────
// ── HE (802.11ax) — carried inside the extension element (255) ──
// An AX200 talking to a Wi-Fi 6 AP gets its operating width from HERE, not
// from the standalone VHT Operation element: `ieee80211_determine_ap_chan`
// (mac80211/mlme.c) takes the 3-byte VHT Operation Information out of the HE
// Operation element whenever the AP carries HE Capability and sets the
// VHT_OPER_INFO bit, and only falls back to element 192 otherwise.
```

## L972-975 · `pub const HE_OP_OFF_PARAMS: usize = 1;        // __le32`

```
// struct ieee80211_he_operation: __le32 he_oper_params, __le16 he_mcs_nss_set,
// u8 optional[]. Offsets are from the element body, i.e. including the leading
// extension-ID byte. ieee80211_he_oper_size: the VHT Operation Information is
// the FIRST optional field, so it starts right after he_mcs_nss_set.
```

## L976 · `pub const HE_OP_OFF_PARAMS: usize = 1;        // __le32`

```
// __le32
```

## L977 · `pub const HE_OP_OFF_VHT_OPER_INFO: usize = 7; // 1 + 4 + 2`

```
// 1 + 4 + 2
```

## L983 · `pub const VHT_CAP_IE_LEN: usize = 12;`

```
/// struct ieee80211_vht_cap: __le32 vht_cap_info, then supp_mcs (8 B).
```

## L985 · `pub const VHT_OFF_CAP_INFO: usize = 0;      // __le32`

```
// __le32
```

## L986 · `pub const VHT_OFF_RX_MCS_MAP: usize = 4;    // __le16`

```
// __le16
```

## L987 · `pub const VHT_OFF_RX_HIGHEST: usize = 6;    // __le16`

```
// __le16
```

## L988 · `pub const VHT_OFF_TX_MCS_MAP: usize = 8;    // __le16`

```
// __le16
```

## L989 · `pub const VHT_OFF_TX_HIGHEST: usize = 10;   // __le16`

```
// __le16
```

## L990 · `pub const VHT_OP_OFF_CHAN_WIDTH: usize = 0;`

```
/// struct ieee80211_vht_operation: chan_width, seg0, seg1, basic_mcs_set.
```

## L993 · `pub const IEEE80211_VHT_CHANWIDTH_USE_HT: u8 = 0;`

```
/// ieee80211.h:2137
```

## L996 · `pub const IEEE80211_VHT_CAP_RXLDPC: u32 = 0x0000_0010;`

```
/// ieee80211.h:2438/2439
```

## L999-1000 · `pub const VHT_MCS_MAP_2SS: u16 = 0xFFFA;`

```
/// Two bits per spatial stream: 2 = MCS 0-9 supported, 3 = stream unused.
/// 2x2 → streams 1 and 2 get 0b10, the other six 0b11.
```

## L1006-1010 · `pub const HT_CAP_IE_LEN: usize = 26;`

```
// ── HT (802.11n) capability element — include/linux/ieee80211.h ──────
// struct ieee80211_ht_cap, 26 bytes: cap_info(2) ampdu_params(1) mcs(16)
// extended_ht_cap(2) tx_BF_cap(4) antenna_selection(1). Without this element in
// the assoc request the AP treats us as an 802.11a/g station: 54 Mbit ceiling,
// no aggregation, and the firmware's TLC has nothing above OFDM to scale into.
```

## L1012 · `pub const HT_OFF_CAP_INFO: usize = 0;   // __le16`

```
// __le16
```

## L1013 · `pub const HT_OFF_AMPDU_PARAMS: usize = 2; // u8`

```
// u8
```

## L1014 · `pub const HT_OFF_MCS_RX_MASK: usize = 3;  // u8[10] (mcs.rx_mask)`

```
// u8[10] (mcs.rx_mask)
```

## L1015 · `pub const HT_OFF_MCS_TX_PARAMS: usize = 3 + 12; // mcs.tx_params (after rx_mask+rx_highest)`

```
// mcs.tx_params (after rx_mask+rx_highest)
```

## L1017 · `pub const IEEE80211_HT_CAP_SM_PS_DISABLED: u16 = 3 << 2; // WLAN_HT_CAP_SM_PS_DISABLED`

```
// WLAN_HT_CAP_SM_PS_DISABLED
```

## L1019 · `pub const IEEE80211_HT_CAP_RX_STBC_1: u16 = 1 << 8;  // 1 spatial stream`

```
// 1 spatial stream
```

## L1025 · `pub const HT_AMPDU_FACTOR_64K: u8 = 3;`

```
// iwl_init_ht_hw_capab: the AX200 advertises 64 KB A-MPDU / 4 us MPDU density.
```

## L1027 · `pub const HT_AMPDU_DENSITY_4US: u8 = 5; // IEEE80211_HT_MPDU_DENSITY_4`

```
// IEEE80211_HT_MPDU_DENSITY_4
```

## L1029-1031 · `pub const WMM_INFO_IE: [u8; 9] = [`

```
// WMM Information Element (WFA vendor-specific 221, OUI 00:50:F2 type 2
// subtype 0 version 1, then a QoS-Info byte). An HT station is by definition a
// QoS station; APs that see HT without WMM may refuse to use HT rates.
```

## L1036-1037 · `pub const DOT11_FC_QOS_DATA: u8 = 0x88; // type data(2), subtype 8 (QoS data)`

```
// QoS data frames: subtype bit 3 set (0x88 with the data type) and a 2-byte QoS
// control field after addr3/seq, so the header is 26 instead of 24 bytes.
```

## L1038 · `pub const DOT11_FC_QOS_DATA: u8 = 0x88; // type data(2), subtype 8 (QoS data)`

```
// type data(2), subtype 8 (QoS data)
```

## L1041-1043 · `pub const DOT11_STYPE_ACTION: u8 = 13;`

```
// Block-Ack action frames (802.11 category 3). Accepting one is what lets the
// AP aggregate: without a session every MPDU pays its own preamble, SIFS and
// ACK, which caps a 115 Mbit link at about a seventh of that.
```

## L1050 · `pub const WLAN_STATUS_UNSPECIFIED_QOS: u16 = 32;`

```
// include/linux/ieee80211.h — verified against 6.18.26, not remembered.
```

## L1057 · `pub const IEEE80211_MAX_AMPDU_BUF_HT: usize = 0x40;`

```
/// The largest reorder window an HT peer may ask for (ieee80211.h:2046).
```

## L1059-1061 · `pub const IEEE80211_MAX_AMPDU_BUF_HE: usize = 0x100;`

```
/// ieee80211.h:2047. An HE peer may run a 256-MPDU reorder window, and iwlwifi
/// reports exactly this as `hw->max_rx_aggregation_subframes` for everything
/// below BZ (mvm/ops.c:1233).
```

## L1063 · `pub const IEEE80211_DELBA_PARAM_TID_MASK: u16 = 0xF000;`

```
/// DELBA parameter set (ieee80211.h:2036) — TID in 15:12, initiator in bit 11.
```

## L1070-1073 · `pub const BA_PARAM_POLICY_IMMEDIATE: u16 = 0x0002;`

```
// ADDBA request/response body (802.11-2020 9.6.3.1): category, action, dialog
// token, then the Block Ack Parameter Set — AMSDU(0), policy(1, 1 = immediate),
// TID(2..5), buffer size(6..15) — a timeout, and for the request a start
// sequence control whose upper 12 bits are the SSN.
```

## L1080 · `pub const DOT11_BEACON_CAP_OFF: usize = DOT11_HDR_LEN + 8 + 2; // 34`

```
// privacy bit in a beacon's capability field (offset hdr+8+2 = 34).
```

## L1081 · `pub const DOT11_BEACON_CAP_OFF: usize = DOT11_HDR_LEN + 8 + 2; // 34`

```
// 34
```

## L1084-1089 · `pub const MAC_CONF_GROUP: u8 = 0x3; // fw/api/commands.h`

```
// ── Stage 5c: session protection (prepare_tx hook before auth) ────
// mac80211 calls drv_mgd_prepare_tx → iwl_mvm_mac_mgd_prepare_tx →
// iwl_mvm_protect_assoc → iwl_mvm_schedule_session_protection right before the
// auth frame. It reserves channel time for the auth/assoc exchange; without it
// the unassociated STA gets no airtime and the FW holds the frame back.
// FW has CAPA_SESSION_PROT_CMD (bit 54) → the SESSION_PROTECTION_CMD path.
```

## L1090 · `pub const MAC_CONF_GROUP: u8 = 0x3; // fw/api/commands.h`

```
// fw/api/commands.h
```

## L1091 · `pub const SESSION_PROTECTION_CMD: u8 = 0x5; // MAC_CONF_GROUP, cmd_ver=1 (fw/api/mac-cfg.h)`

```
// MAC_CONF_GROUP, cmd_ver=1 (fw/api/mac-cfg.h)
```

## L1092 · `pub const SP_CMD_LEN: usize = 24;`

```
// struct iwl_session_prot_cmd (fw/api/time-event.h), 24 B, all __le32.
```

## L1094 · `pub const SP_OFF_ID_COLOR: usize = 0;  // mac id (cmd_ver 1 → mvmvif->id = 0)`

```
// mac id (cmd_ver 1 → mvmvif->id = 0)
```

## L1095 · `pub const SP_OFF_ACTION: usize = 4;    // FW_CTXT_ACTION_ADD`

```
// FW_CTXT_ACTION_ADD
```

## L1096 · `pub const SP_OFF_CONF_ID: usize = 8;   // SESSION_PROTECT_CONF_ASSOC = 0`

```
// SESSION_PROTECT_CONF_ASSOC = 0
```

## L1097 · `pub const SP_OFF_DURATION_TU: usize = 12; // MSEC_TO_TU(900) = 900*1000/1024 = 878`

```
// MSEC_TO_TU(900) = 900*1000/1024 = 878
```

## L1098 · `pub const SESSION_PROTECT_CONF_ASSOC: u32 = 0; // first enum value`

```
// repetition_count @16, interval @20 — not used, 0.
```

## L1099 · `pub const SESSION_PROTECT_CONF_ASSOC: u32 = 0; // first enum value`

```
// first enum value
```

## L1100 · `pub const SP_DURATION_TU: u32 = 878; // schedule_session_protection passes 900 ms`

```
// schedule_session_protection passes 900 ms
```

## L1102-1107 · `pub const MAC_PM_POWER_TABLE: u8 = 0xa9;`

```
// ── Connect tail: per-MAC power (iwl_mvm_power_update_mac) ────────
// __iwl_mvm_assign_vif_chanctx sends power before quotas ("Power state must be
// updated before quotas"). iwl_mvm_power_send_cmd → MAC_PM_POWER_TABLE (0xa9,
// legacy group → promoted to LONG_GROUP). struct iwl_mac_power_cmd, 40 B. We
// model the power-save-disabled path (iwl_mvm_power_build_cmd early-return):
// only id_and_color + keep_alive_seconds, flags = 0 (no PS — we don't sleep).
```

## L1110 · `pub const MP_OFF_ID_COLOR: usize = 0;     // __le32 FW_CMD_ID_AND_COLOR(0,0)=0`

```
// __le32 FW_CMD_ID_AND_COLOR(0,0)=0
```

## L1111 · `pub const MP_OFF_FLAGS: usize = 4;        // __le16 (0 = PS disabled)`

```
// __le16 (0 = PS disabled)
```

## L1112 · `pub const MP_OFF_KEEP_ALIVE: usize = 6;   // __le16 keep_alive_seconds`

```
// __le16 keep_alive_seconds
```

## L1113 · `pub const POWER_KEEP_ALIVE_PERIOD_SEC: u16 = 25; // max(3*dtim*bi, this); dtim=0 → 25`

```
// max(3*dtim*bi, this); dtim=0 → 25
```

## L1115-1118 · `pub const ETHERTYPE_EAPOL: u16 = 0x888E;`

```
// ── Stage 5e: EAPOL transport + WiFi-class control channel ───────
// After association the AP runs the WPA2 4-way handshake; its EAPOL-Key frames
// arrive as 802.11 DATA frames (LLC/SNAP, ethertype 0x888E). We demux them off
// the RX ring and forward to wifid (the supplicant) over the control channel.
```

## L1121 · `pub const DOT11_FC_TYPE_DATA: u8 = 0x08; // fc byte0 & 0x0c == data`

```
// fc byte0 & 0x0c == data
```

## L1122 · `pub const DOT11_FC_PROTECTED: u8 = 0x40;  // fc byte1 — payload is encrypted`

```
// fc byte1 — payload is encrypted
```

## L1123 · `pub const DOT11_STYPE_QOS: u8 = 0x08;    // subtype bit → +2-byte QoS control`

```
// subtype bit → +2-byte QoS control
```

## L1124-1125 · `pub const DOT11_STYPE_NODATA: u8 = 0x04;`

```
/// Data subtypes with this bit carry NO BODY — Null (4) and QoS Null (12).
/// `ieee80211_is_nullfunc` / `ieee80211_is_qos_nullfunc` in Linux.
```

## L1127 · `pub const CMD_SET_KEY: u8 = 0x04;`

```
// Control-channel wire ops (docs/spec/WIFI_CLASS_ABI.md). downlink = manager→driver.
```

## L1132 · `pub const EV_EAPOL_RX: u8 = 0x84;`

```
// uplink = driver→manager.
```

## L1138 · `pub const IWL_DATA_TID: u8 = 0;`

```
// ── Stage 5e: data TX queue (tid 0) + key install ────────────────
```

## L1140-1160 · `pub const IWL_DATA_QUEUE_SIZE: usize = 256;`

```
// Data TX queue depth. 256 is what Linux gives this hardware, and the number is
// derived, not chosen. `iwl_mvm_get_queue_size` (mvm/sta.c:812) asks for 1024 on
// an HE peer — ours is one — but the transport clamps it twice in
// `iwl_txq_dyn_alloc` (pcie/gen1_2/tx-gen2.c:1033):
//
//     size = min(size, bc_tbl_size / sizeof(u16));   // 320 on this family
//     size = rounddown_pow_of_two(size);             // -> 256
//
// `bc_tbl_size` follows `TFD_QUEUE_BC_SIZE` = TFD_QUEUE_SIZE_MAX + BC_DUP =
// 256 + 64 (iwl-fh.h:594), and cfg/22000.c:29 sets `.max_tfd_queue_size = 256`.
// 256 is also the ceiling of the reclaim path: the TX response states the read
// pointer in 8 bits of the header sequence (`SEQ_TO_INDEX`), so a deeper queue
// would alias. Three independent limits, all at 256.
//
// 64 was our own number. Measured at 74 Mbit with the byte cap in place
// (0.98.0): the byte cap never fired once, the RING guard fired 4565 times and
// sat at peak 62/62, and fq_codel dropped 2292 of 21487 ACKs the driver could
// not take. The queue depth WAS the throughput limit, and it said so itself.
//
// The TFD slot index = write_ptr & (size-1); cb_size = ilog2(size)-3 (must
// match the size). Costs 592 KB of DMA against 148 KB at 64 slots.
```

## L1163-1186 · `pub const AQL_LIMIT_LOW_US: u32 = 5_000;    // IEEE80211_DEFAULT_AQL_TXQ_LIMIT_L`

```
// ── AQL — Airtime Queue Limits (net/mac80211) ────────────────────────────
//
// The anti-bufferbloat cap on the data queue, and it counts MICROSECONDS OF
// AIRTIME, not bytes and not frames.
//
// It was a frame count until 0.98.0 and a byte count until 0.104.0. Both were
// our own numbers, and both were wrong for the same reason: what a queued
// frame costs the medium is a TIME, and that time depends on the rate the
// firmware happens to be using. 43 full-size frames are 52 us each at
// 240 Mbit and 2069 us each at 6 Mbit — the same byte cap is a reasonable
// queue in one case and two and a half seconds of bufferbloat in the other.
// Linux solved this in 2019 and the mechanism has a name.
//
// `ieee80211_txq_airtime_check` (mac80211/tx.c:4164) admits a frame when
//
//     pending < aql_limit_low                                      -> yes
//     total_pending < aql_threshold && pending < aql_limit_high     -> yes
//     otherwise                                                     -> no
//
// with the defaults from include/net/cfg80211.h:3602. We have ONE station and
// in practice one AC, so `total_pending` and this station's `pending` are the
// same counter and the 24000 ceiling can never bind before the 12000 one —
// the branch is kept anyway, so the day a second station exists it is already
// right.
```

## L1187 · `pub const AQL_LIMIT_LOW_US: u32 = 5_000;    // IEEE80211_DEFAULT_AQL_TXQ_LIMIT_L`

```
// IEEE80211_DEFAULT_AQL_TXQ_LIMIT_L
```

## L1188 · `pub const AQL_LIMIT_HIGH_US: u32 = 12_000;  // IEEE80211_DEFAULT_AQL_TXQ_LIMIT_H`

```
// IEEE80211_DEFAULT_AQL_TXQ_LIMIT_H
```

## L1189 · `pub const AQL_THRESHOLD_US: u32 = 24_000;   // IEEE80211_AQL_THRESHOLD`

```
// IEEE80211_AQL_THRESHOLD
```

## L1191-1192 · `pub const AQL_AVG_PKT_SIZE: u32 = 1024;`

```
/// `AVG_PKT_SIZE` (mac80211/airtime.c:11). The rate tables are built for a
/// packet of this size and scaled to the real length.
```

## L1195-1196 · `pub const AQL_LEN_OVERHEAD: u32 = 38;`

```
/// `len += 38` at the top of `ieee80211_calc_expected_tx_airtime` — the
/// Ethernet header allowance it adds before doing anything else.
```

## L1199 · `pub const AQL_MIN_US: u32 = 4;`

```
/// Lower bound of `ieee80211_calc_expected_tx_airtime`: `max_t(u32, duration, 4)`.
```

## L1202-1215 · `pub const TX_INFLIGHT_MAX: u32 = IWL_DATA_QUEUE_SIZE as u32 - 2;`

```
/// Ring guard, not a policy: the write pointer must never lap the firmware's
/// read pointer. AQL is what limits us in practice; this is the wall behind it,
/// and it binds only when the airtime estimate is so small that thousands of
/// frames would fit in 12 ms. Must stay < QUEUE_SIZE-1.
///
/// History of the frame cap this replaced, kept because the numbers were
/// measured on the device and the next size change has to beat them:
///
///     Deckel 16:  blocked 4100, fq_codel verwarf 3267 ("driver too slow")
///     Deckel 32:  46 Mbit (Server 53)  retrans 101
///     Deckel 48:  57 Mbit (Server 69)  retrans 5  ssthresh 364  rtt 65 ms
///
/// Schon damals notiert: "fq_codels Plaetze laufen ueber, waehrend der Treiber
/// am Deckel steht. Der naechste Hebel liegt dort, nicht bei dieser Zahl."
```

## L1217-1239 · `pub const TX_WD_TIMEOUT_MS: u64 = 10_000;`

```
/// TX queue watchdog. Linux arms it whenever the queue is NOT EMPTY and pushes
/// it forward on every completion; it does not care how full the queue is.
///
/// The INTERVAL is deliberately not Linux's: `IWL_LONG_WD_TIMEOUT` is 10 s
/// (`cfg/22000.c:32`) because Linux's reaction is to force an NMI and restart
/// the firmware — a sledgehammer you want to be very sure about. Ours only
/// hands leaked slots back, and it cannot lap the firmware: the ring holds 256
/// TFDs while `TX_INFLIGHT_MAX` caps us at 16, so even an early reclaim leaves
/// ~32 of 256 outstanding at worst.
///
/// Measured on the device at 10 s: four leaked slots cost a 100 MB transfer
/// 26 s for what the server sent in 9.2 s, with the server reporting a 333 ms
/// RTT — a third of a second of ACKs we could not send. A frame that has not
/// been acknowledged after a second is lost; the firmware's own retry sequence
/// is over long before that.
/// Linux: `cfg/22000.c` sets `.wd_timeout = IWL_LONG_WD_TIMEOUT` = 10000 for
/// this family (iwl-config.h:87). Ours was 1000 — our own number, from the
/// commit that "lowered it from 10 s to 1 s". On a saturated channel a frame
/// legitimately takes more than a second to get out, so the watchdog declared
/// a healthy queue stuck and set `data_in_flight = 0` — which is not true, the
/// slots are not free — and then we oversubscribed the queue on top of an
/// already busy link. Measured during an OTA over WiFi: it fired again and
/// again while the update crawled. Back to the value Linux uses.
```

## L1241 · `pub const DATA_QUEUE_CB_SIZE: u32 = 5; // TFD_QUEUE_CB_SIZE(256) = ilog2(256)-3`

```
// TFD_QUEUE_CB_SIZE(256) = ilog2(256)-3
```

## L1242-1245 · `pub const TX_PAYLOAD_STRIDE: usize = 2048;`

```
// Per-slot TX staging stride. Each in-flight TFD's TB1 must point at its OWN
// payload region, or back-to-back frames clobber each other's data before the
// firmware DMAs it (the bug behind "dies under load once the rate went up").
// One full dev-cmd + 802.11 data frame (24 + ~1532) fits in 2 KiB.
```

## L1247-1248 · `pub const ADD_STA_KEY_CMD: u8 = 0x17;`

```
// ADD_STA_KEY (0x17 LEGACY → LONG_GROUP, cmd_ver 3). struct iwl_mvm_add_sta_key_cmd
// = common(52) + rx_mic(8) + tx_mic(8) + tx_seq(8) = 76 B. CCMP: mic/seq all 0.
```

## L1251 · `pub const KEY_OFF_STA_ID: usize = 0;       // u8`

```
// u8
```

## L1252 · `pub const KEY_OFF_KEY_OFFSET: usize = 1;   // u8 (FW key-table slot)`

```
// u8 (FW key-table slot)
```

## L1253 · `pub const KEY_OFF_KEY_FLAGS: usize = 2;    // __le16`

```
// __le16
```

## L1254 · `pub const KEY_OFF_KEY: usize = 4;          // u8[32]`

```
// u8[32]
```

## L1255 · `pub const KEY_OFF_RX_SEQ: usize = 36;      // u8[16] (rx_secur_seq_cnt / RSC)`

```
// u8[16] (rx_secur_seq_cnt / RSC)
```

## L1256 · `pub const STA_KEY_FLG_CCM: u16 = 2 << 0;`

```
// iwl_sta_key_flag: CCMP encryption + key id + group/MFP.
```

## L1259-1260 · `pub const STA_KEY_MAX_NUM: u8 = 16;`

```
/// Firmware key-table size, `fw/api/sta.h:190`. We used exactly two of the
/// sixteen — slot 0 for the pairwise key, slot 1 for EVERY group key.
```

## L1264 · `pub const DOT11_FC_DATA: u8 = 0x08; // type data, subtype 0`

```
// 802.11 data frame: frame_control byte0 = data(type 2), byte1 toDS for STA→AP.
```

## L1265 · `pub const DOT11_FC_DATA: u8 = 0x08; // type data, subtype 0`

```
// type data, subtype 0
```

## L1268-1280 · `pub const BA_NOTIF: u8 = 0xc5;`

```
// ── TX completion (struct iwl_tx_resp, fw/api/tx.h) ──────────────
// The new-tx-api variant, which is what a gen2 device sends
// (iwl_mvm_get_agg_status → &((struct iwl_tx_resp *)tx_resp)->status). 44 bytes;
// offsets relative to pkt->data (RX_PKT_DATA_OFF). This is the only place the
// host learns what an on-air transmission actually cost: how many times it was
// retried, and how many microseconds of airtime it burned.
// ── BA_NOTIF (0xc5, group 0) — struct iwl_compressed_ba_notif ──────
// With TLC offload the FIRMWARE runs the TX aggregation manager: mac80211 sets
// IEEE80211_HW_TX_AMPDU_SETUP_IN_HW and ieee80211_start_tx_ba_session refuses
// to do it in software (agg-tx.c:628). The host never sends an ADDBA request
// and never clears tid_disable_tx — Linux keeps 0xffff on this firmware too.
// What the host DOES get is this notification per aggregate, and it is the
// only place we can see whether our transmissions are being aggregated at all.
```

## L1282 · `pub const CBA_OFF_STA_ID: usize = 4;      // u8`

```
// u8
```

## L1283 · `pub const CBA_OFF_TXED: usize = 14;       // __le16 — MPDUs sent in the aggregate`

```
// __le16 — MPDUs sent in the aggregate
```

## L1284 · `pub const CBA_OFF_DONE: usize = 16;       // __le16 — MPDUs acknowledged`

```
// __le16 — MPDUs acknowledged
```

## L1285 · `pub const CBA_OFF_RTS_RETRY: usize = 18;  // u8`

```
// u8
```

## L1286 · `pub const CBA_OFF_WIRELESS_TIME: usize = 20; // __le32`

```
// __le32
```

## L1287 · `pub const CBA_OFF_TFD_CNT: usize = 28;    // __le16`

```
// __le16
```

## L1288 · `pub const CBA_HDR_LEN: usize = 32;        // flex array starts here`

```
// flex array starts here
```

## L1289 · `pub const CBA_TFD_Q_NUM: usize = 0;       // __le16`

```
// struct iwl_compressed_ba_tfd, 8 bytes each
```

## L1290 · `pub const CBA_TFD_Q_NUM: usize = 0;       // __le16`

```
// __le16
```

## L1291 · `pub const CBA_TFD_INDEX: usize = 2;       // __le16 — the queue's new read ptr`

```
// __le16 — the queue's new read ptr
```

## L1292 · `pub const CBA_TFD_TID: usize = 5;         // u8`

```
// u8
```

## L1294-1296 · `pub const CBA_TFD_MAX: usize = 16;`

```
/// Entries of the flex array we walk. `tfd_cnt` is one per (queue, TID) the
/// aggregate touched, so 8 TIDs is the real ceiling; 16 is slack. Truncation is
/// counted, never silent — an unread entry is a TFD slot that never comes back.
```

## L1300 · `pub const TXR_OFF_FRAME_COUNT: usize = 0;  // u8`

```
// u8
```

## L1301 · `pub const TXR_OFF_FAILURE_RTS: usize = 2;  // u8`

```
// u8
```

## L1302 · `pub const TXR_OFF_FAILURE_FRAME: usize = 3; // u8 — retries; count = this + 1`

```
// u8 — retries; count = this + 1
```

## L1303 · `pub const TXR_OFF_INITIAL_RATE: usize = 4; // __le32 rate_n_flags`

```
// __le32 rate_n_flags
```

## L1304 · `pub const TXR_OFF_MEDIA_TIME: usize = 8;   // __le16 wireless_media_time (us)`

```
// __le16 wireless_media_time (us)
```

## L1305 · `pub const TXR_OFF_BYTE_CNT: usize = 30;    // __le16`

```
// __le16
```

## L1306 · `pub const TXR_OFF_STATUS: usize = 40;      // __le16 status.status`

```
// __le16 status.status
```

## L1310 · `pub const RATE_MCS_SGI_MSK: u32 = 1 << 20;`

```
// rate_n_flags v2, remaining fields we decode for the report (fw/api/rs.h).
```

## L1314-1318 · `pub const WIFI_CFG_PATH: &str = "sys/config/wifi";`

```
// ── Connect policy (sys/config/wifi) ──
// One file, `key: value` per line, same shape as `sys/config/bar`. Keys:
// ssid, band, ampdu, ps, btcoex, settle_ms. The passphrase stays in its own
// object (`sys/config/wifi_psk`): only wifid needs it, and nothing else should
// hold it in a buffer.
```

## L1321 · `pub const BAND_PREF_AUTO: u8 = 0; // prefer 5 GHz when it is strong enough`

```
// prefer 5 GHz when it is strong enough
```

## L1322 · `pub const BAND_PREF_5: u8 = 1;    // 5 GHz only (fall back if none)`

```
// 5 GHz only (fall back if none)
```

## L1323 · `pub const BAND_PREF_24: u8 = 2;   // 2.4 GHz only (fall back if none)`

```
// 2.4 GHz only (fall back if none)
```

## L1324-1327 · `pub const BAND_PREF_5_MIN_RSSI: i8 = -60;`

```
// Above this RSSI a 5 GHz AP is worth taking over a louder 2.4 GHz one: the
// band is uncongested and, in a repeater mesh, usually the router itself rather
// than a node whose backhaul halves the throughput. wpa_supplicant's band
// preference works the same way — a signal floor, not a pure RSSI contest.
```

## L1329-1333 · `pub const BAND_PREF_5_MAX_PENALTY_DB: i16 = 12;`

```
// …and never more than this far below the best 2.4 GHz AP of the same network.
// A floor alone is not enough: 5 GHz at -64 dBm clears any sane floor while a
// 2.4 GHz radio of the same mesh sits at -46, and the wider band cannot make up
// an 18 dB deficit. Measured case: at -64 not even a beacon of the chosen BSS
// arrived, so the association completed and then nothing else ever did.
```

## L1340-1344 · `pub const REPORT_CAP: usize = 3072;`

```
// Status snapshot published via npk_driver_report. One screen of text.
// The kernel accepts REPORT_MAX = 4096; staying well under it costs nothing and
// the report has outgrown 1600 — at which point `s()` silently stopped writing
// and the last lines (sync, scan) vanished without a trace. Truncation now says
// so, because a report that quietly ends early is worse than no report.
```

## L1346-1347 · `pub const REPORT_PERIOD_MS: u64 = 1000;`

```
// How often the snapshot is refreshed. 1 s is short enough to show a speed test
// live and long enough that formatting never lands in the hot path.
```

## L1350 · `pub const PCI_CAP_PTR: u8 = 0x34; // first capability pointer`

```
// ── PCIe capability layout (apm_config: ASPM / LTR detect) ───────
```

## L1351 · `pub const PCI_CAP_PTR: u8 = 0x34; // first capability pointer`

```
// first capability pointer
```

## L1352 · `pub const PCI_CAP_ID_EXP: u8 = 0x10; // PCI Express capability`

```
// PCI Express capability
```

## L1353 · `pub const PCI_EXP_LNKCTL: u8 = 0x10; // offset within PCIe cap`

```
// offset within PCIe cap
```

## L1354 · `pub const PCI_EXP_DEVCTL2: u8 = 0x28; // offset within PCIe cap`

```
// offset within PCIe cap
```

## L1358-1361 · `pub const HANDSHAKE_TIMEOUT_MS: u64 = 8000;`

```
// How long to wait for the AP to start the 4-way after associating before
// treating the association as dead and reconnecting. An AP normally sends msg1
// within milliseconds; 8 s leaves room for a retransmit round without leaving a
// silently-dead link up for minutes.
```

## L1364 · `pub const BT_COEX_DISABLE: u32 = 0x0; // iwlwifi.bt_coex_active=0`

```
// iwlwifi.bt_coex_active=0
```

## L1366-1367 · `pub const RX_SILENCE_MS: u64 = 3000;`

```
// Silence on the RX ring that cannot be the air: even an idle channel carries
// beacons at ~10/s. Longer than this means the firmware has no buffer to fill.
```

## L1370-1372 · `pub const BA_SETUP_MS: u64 = 300;`

```
// How long the firmware gets to answer a block-ack setup before we tell the
// AP no. Generous: the command queue is shallow and the answer is normally
// one poll pass away.
```

## L1375 · `pub const SETTLE_MS_DEFAULT: u32 = 4000;`

```
// Pause between firmware bring-up and the first scan (`settle_ms` in sys/config/wifi).
```

