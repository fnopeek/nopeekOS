# WiFi TX Pipeline — Linux-Audit (rtw8852be, chip-gen AX)

Strict Linux-Port-Referenz für Mgmt-TX (Probe Request, AUTH, ASSOC).
Quelle: `/tmp/linux-rtw89/drivers/net/wireless/realtek/rtw89/` — mainline.

Ziel Etappe 1: **Probe Request auf CH8 senden, Probe Response empfangen** — Proof-of-Concept für TX-Pfad.

Legende: `[x]` da · `[/]` partiell · `[ ]` fehlt · `[·]` nicht nötig für 8852BE

---

## 0 — Chip-Parameter (rtw8852be.c:12)

| Feld | Wert (8852BE) | Bedeutung |
|------|---------------|-----------|
| chip_gen | `RTW89_CHIP_AX` | AX-Format (non-V1 TXWD-Body, non-V2 BE) |
| `dma_addr_set` | `rtw89_pci_ch_dma_addr_set` | non-V1 Register-Layout |
| `bd_ram_table` | `rtw89_bd_ram_table_single` | Single-Band |
| `fill_txaddr_info` | `rtw89_pci_fill_txaddr_info` | non-V1 Addr-Info-Format |
| `tx_dma_ch_mask` | `ACH4-7 \| CH10 \| CH11` | **disabled** |
| `dma_stop1` | `{0x1010, B_AX_TX_STOP1_MASK_V1}` | DMA enable für aktive Channels |
| `txwd_body_size` | `sizeof(rtw89_txwd_body)` = **24 B** (6 dwords) | AX body |
| `txwd_info_size` | `sizeof(rtw89_txwd_info)` = **24 B** (6 dwords) | AX info |
| `h2c_desc_size` | `sizeof(rtw89_txwd_body)` = 24 B | FW-CMD header (CH12) |

Aktive TX-Channels (nach Mask):
```
ACH0 (BE)  ACH1 (BK)  ACH2 (VI)  ACH3 (VO)  — Data queues
CH8  (MGMT Band 0)    CH9 (HI Band 0)        — Management
CH12 (FW CMD)                                — haben wir
```

---

## 1 — TXBD Ring (PCIe Descriptor Ring)

### Struct (`pci.h:1461`)

```c
struct rtw89_pci_tx_bd_32 {
    __le16 length;   // 0..1: WD-Page length (bytes)
    __le16 opt;      // 2..3:
                     //   bit 14  = LS   (Last Segment)
                     //   bit 6-13 = DMA_HI (upper 8 bits of DMA addr)
    __le32 dma;      // 4..7: DMA addr low 32 bits (points to WD-Page)
};  // 8 bytes total
```

### Ring-Sizes (`pci.h:1118-1121`)

```
RTW89_PCI_TXBD_NUM_MAX     = 256   (BDs per channel)
RTW89_PCI_TXWD_NUM_MAX     = 512   (WD pages per channel)
RTW89_PCI_TXWD_PAGE_SIZE   = 128   (bytes per WD page)
```

Ring-Memory je Channel: BD-Ring = 256×8 = 2KB, WD-Ring = 512×128 = 64KB.

### Register pro Channel (non-V1, für 8852BE)

| Channel | `_TXBD_NUM` | `_TXBD_IDX` | `_BDRAM_CTRL` | `_TXBD_DESA_L` | `_TXBD_DESA_H` |
|---------|-------------|-------------|---------------|----------------|----------------|
| ACH0    | 0x1024 (16b)| 0x1058 (16b)| 0x1200 (32b)  | 0x1110         | 0x1114         |
| ACH1    | 0x1026      | 0x105C      | 0x1204        | 0x1118         | 0x111C         |
| ACH2    | 0x1028      | 0x1060      | 0x1208        | 0x1120         | 0x1124         |
| ACH3    | 0x102A      | 0x1064      | 0x120C        | 0x1128         | 0x112C         |
| **CH8** | **0x1034**  | **0x1078**  | **0x1220**    | **0x1150**     | **0x1154**     |
| CH9     | 0x1036      | 0x107C      | 0x1224        | 0x1158         | 0x115C         |
| CH12    | 0x1038      | 0x1080      | 0x1228        | 0x1160         | 0x1164         |

Reset-Register:
- `R_AX_TXBD_RWPTR_CLR1 = 0x1014` — bit 8 (`CLR_CH8_IDX`) cleart wp/rp für CH8

### BDRAM Table single-band (`pci.c:1711`)

```
[ACH0] start=0,  max=5, min=2
[ACH1] start=5,  max=5, min=2
[ACH2] start=10, max=5, min=2
[ACH3] start=15, max=5, min=2
[CH8]  start=20, max=4, min=1   ← für uns
[CH9]  start=24, max=4, min=1
[CH12] start=28, max=4, min=1   ← haben wir
```

BDRAM schreiben via: `SIDX<<0 | MAX<<8 | MIN<<16` ins `_BDRAM`-Register.

---

## 2 — TXWD Page Layout (DMA-Memory für ein Frame)

Eine WD-Page (128 Bytes) enthält in dieser Reihenfolge:

```
Offset 0              TXWD Body       24 B  (6 dwords, AX)
Offset 24             TXWD Info       24 B  (6 dwords, wenn en_wd_info=1)
Offset 48 (oder 24)   TXWP Info        8 B  (seq0..3)
Offset 56 (oder 32)   Addr Info       8×N B (non-V1: 8 B pro Entry)
— Rest der Page frei —

Das eigentliche 802.11-Frame liegt in einem SEPARATEN DMA-Buffer.
Addr-Info zeigt per DMA-Pointer darauf.
```

### TXWP Info (`pci.h:1471`, 8 Bytes)

```c
struct rtw89_pci_tx_wp_info {
    __le16 seq0;  // (txwd->seq) | BIT(15) RTW89_PCI_TXWP_VALID
    __le16 seq1;  // 0
    __le16 seq2;  // 0
    __le16 seq3;  // 0
};
```

`txwd->seq` ist der Page-Index im WD-Ring (0..511).

### Addr Info non-V1 (`pci.h:1483`, 8 Bytes pro Entry)

```c
struct rtw89_pci_tx_addr_info_32 {
    __le16 length;   // Frame length
    __le16 option;   // bit 15 MSDU_LS + NUM(1) | (dma_hi << 6)
    __le32 dma;      // Frame DMA addr low 32 bits
};
```

Für ein Single-Buffer-Frame (unser Fall): **1 Entry**, `MSDU_LS | NUM(1)`.

---

## 3 — TX-Pipeline (pci.c:1494 `rtw89_pci_txwd_submit`)

```
1. dma_map_single(frame_data, frame_len) → frame_dma
2. WD-Page allozieren aus wd_ring->free_pages → txwd (128 B DMA-coherent)
3. In WD-Page schreiben:
   a. TXWP Info @offset (body+info): seq0 = txwd->seq | VALID
   b. Addr-Info @offset (body+info+wp): length/option/dma → frame_dma
   c. TXWD-Body + TXWD-Info via rtw89_core_fill_txdesc(desc_info, txwd->vaddr)
4. txwd->len = body_size + info_size + 8 + addr_info_len
5. txwd zu busy_pages hinzufügen (für TX-completion-reclaim)
6. BD im BD-Ring schreiben (pci.c:1590):
   a. length  = txwd->len
   b. opt     = LS | (txwd->paddr_hi << 6)
   c. dma     = txwd->paddr (low 32 bit, Page-Adresse)
7. bd_ring->wp++ (lokal)
8. Kick-Off: write16(R_AX_CH8_TXBD_IDX, wp)  ← HW liest ab hier
```

---

## 4 — TXWD Body AX (24 Bytes, `core.c:1577 rtw89_core_fill_txdesc`)

**WICHTIG**: AX schreibt nur `dword0`, `dword2`, `dword3`. `dword1`, `dword4`, `dword5` bleiben 0 (memset).

### dword0 (`rtw89_build_txwd_body0`, `txrx.h:68`)

| Bits | Feld | Wert für Probe Req |
|------|------|---------------------|
| 31-24 | WP_OFFSET | 0 |
| 23 | MORE_DATA | 0 |
| 22 | WD_INFO_EN | **1** (wir wollen use_rate) |
| 20 | FW_DL | 0 |
| 19-16 | CHANNEL_DMA | **8** (CH8) |
| 15-11 | HDR_LLC_LEN | 0 für Mgmt (kein LLC/SNAP) |
| 10 | STF_MODE | 0 |
| 7 | WD_PAGE | **1** |
| 5 | HW_AMSDU | 0 |
| 3-2 | HW_SSN_SEL | **1** (`RTW89_MGMT_HW_SSN_SEL`) |
| 1-0 | HW_SSN_MODE | **1** (`RTW89_MGMT_HW_SEQ_MODE`) |

### dword2 (`rtw89_build_txwd_body2`)

| Bits | Feld | Wert |
|------|------|------|
| 30-24 | MACID | **0** (unser macid) |
| 23 | TID_INDICATE | 0 |
| 22-17 | QSEL | **0x12** (`B0_MGMT`) |
| 13-0 | TXPKT_SIZE | **skb->len** (Frame-Länge) |

### dword3 (`rtw89_build_txwd_body3`)

| Bits | Feld | Wert |
|------|------|------|
| 13 | BK | 0 |
| 12 | AGG_EN | 0 |
| 11-0 | SW_SEQ | 0 (HW füllt via HW_SSN_SEL) |

---

## 5 — TXWD Info AX (24 Bytes, nur wenn `en_wd_info=1`)

### dword0 (`rtw89_build_txwd_info0`, `txrx.h:117`)

| Bits | Feld | Wert für Probe Req |
|------|------|---------------------|
| 30 | USE_RATE | **1** |
| 29-28 | DATA_BW | **0** (20 MHz) |
| 27-25 | GI_LTF | 0 |
| 24-16 | DATA_RATE | **0x0** (`RTW89_HW_RATE_CCK1`, 1 Mbps für 2.4G; 0x4 = OFDM6 wenn NoCCK) |
| 15 | DATA_ER | 0 |
| 12 | DATA_STBC | 0 |
| 11 | DATA_LDPC | 0 |
| 10 | DISDATAFB | **1** (no rate fallback) |
| 6-4 | MULTIPORT_ID | 0 (port) |

### dword1 (`rtw89_build_txwd_info1`)

| Bits | Feld | Wert |
|------|------|------|
| 31 | DATA_TXCNT_LMT_SEL | 0 |
| 30-25 | DATA_TXCNT_LMT | 0 |
| 24-16 | DATA_RTY_LOWEST_RATE | 0 |
| 14 | A_CTRL_BSR | 0 |
| 7-0 | MAX_AGGNUM | 0 |

### dword2 (`rtw89_build_txwd_info2`)

| Bits | Feld | Wert |
|------|------|------|
| 20-18 | AMPDU_DENSITY | 0 |
| 12-9 | SEC_TYPE | 0 (no crypto) |
| 8 | SEC_HW_ENC | 0 |
| 7-0 | SEC_CAM_IDX | 0 |

### dword3 (`rtw89_build_txwd_info3`)

| Bits | Feld | Wert |
|------|------|------|
| 10 | SPE_RPT | 0 oder 1 (TX report enable) |

### dword4 (`rtw89_build_txwd_info4`)

| Bits | Feld | Wert für Broadcast Probe |
|------|------|---------------------------|
| 31 | HW_RTS_EN | 1 |
| 27 | RTS_EN | **0** (is_bmc = true → kein RTS) |
| 3-0 | SW_DEFINE | 0 |

---

## 6 — HW-Rate-Konstanten (`core.h:335`)

| Konstante | Hex | Bedeutung |
|-----------|-----|-----------|
| CCK1      | 0x0 | 1 Mbps DSSS (2.4G) |
| CCK2      | 0x1 | 2 Mbps |
| CCK5      | 0x2 | 5.5 Mbps |
| CCK11     | 0x3 | 11 Mbps |
| OFDM6     | 0x4 | 6 Mbps (5G default, 2.4G wenn NoCCK) |
| OFDM12    | 0x6 | ... |
| OFDM24    | 0x8 | ... |
| MCS0      | 0x80 | HT MCS0 |

Default für Mgmt-Frame auf 2.4G ohne spezielle Flags: **CCK1 (0x0)**.

---

## 7 — mgmt_info Setup (`core.c:845 rtw89_core_tx_update_mgmt_info`)

Linux setzt für jeden Mgmt-Frame in `desc_info`:

```
qsel          = RTW89_TX_QSEL_B0_MGMT (0x12)   (oder B0_HI wenn hiq)
ch_dma        = RTW89_TXCH_CH8                  (aus qsel via get_ch_dma)
sw_mld        = true                            (auch ohne MLD)
port          = 0                               (hiq=0 → port=0)
mac_id        = 0                               (unser vif macid)
hw_ssn_sel    = 1 (RTW89_MGMT_HW_SSN_SEL)
hw_seq_mode   = 1 (RTW89_MGMT_HW_SEQ_MODE)
en_wd_info    = true
use_rate      = true
dis_data_fb   = true
data_rate     = CCK1 (2.4G) / OFDM6 (5G/NoCCK)
sec_en        = false (Mgmt unverschlüsselt)
```

---

## 8 — DMA Enable (wir haben ✓ aber prüfen)

Register **R_AX_PCIE_DMA_STOP1 (0x1010)**:

```
bit 20  STOP_PCIEIO   (IO-block)
bit 19  STOP_WPDMA
bit 18  STOP_CH12     (FW CMD)
bit 17  STOP_CH9      (HI Band 0)
bit 16  STOP_CH8      (MGMT Band 0)   ← für uns
bit 15-8 STOP_ACH7..ACH0
```

**8852BE Mask** `B_AX_TX_STOP1_MASK_V1`:
```
ACH0|ACH1|ACH2|ACH3|CH8|CH9|CH12
= bits 8|9|10|11|16|17|18
= 0x00070F00
```

→ **DMA enable** = `write32_clr(0x1010, 0x00070F00)` + clr PCIEIO/WPDMA.

Aktuell im Code (`mac.rs:904`): `mmio_clr32(0x1010, 0x000F_FF00)` — clrs alle bits 8-19. Das enabled **mehr** als nötig (auch ACH4-7 und dev. Ch10/11), aber: 8852BE hat diese Kanäle nicht (tx_dma_ch_mask). Das Clearing tut nix, da kein Ring dort konfiguriert. Aktuell OK, aber Linux-strict wäre 0x00070F00.

---

## 9 — Probe Request Frame (IEEE 802.11-2020 §9.3.3.10)

802.11 MAC Header (24 Bytes für Probe Req):

```
0..1   Frame Control  = 0x0040  (Type=Mgmt, Subtype=ProbeReq, ToDS/FromDS=0)
2..3   Duration       = 0
4..9   addr1  DA      = FF:FF:FF:FF:FF:FF  (Broadcast)
10..15 addr2  SA      = <unsere MAC>
16..21 addr3  BSSID   = FF:FF:FF:FF:FF:FF  (Wildcard)
22..23 Seq Ctrl       = 0 (HW füllt via HW_SSN)
```

Body:
```
IE SSID (0):         len 0 (Wildcard-SSID, zeigt "alle SSIDs")
  byte 0  = 0  (tag SSID)
  byte 1  = 0  (length = 0)

IE Supported Rates (1): 1, 2, 5.5, 11, 6, 9, 12, 18 Mbps
  byte 0  = 1  (tag)
  byte 1  = 8  (len)
  bytes 2-9 = 0x82, 0x84, 0x8B, 0x96, 0x0C, 0x12, 0x18, 0x24
             (× 2 in 500kbps units; bit 7 = basic rate)

IE Extended Supported Rates (50): 24, 36, 48, 54 Mbps
  byte 0  = 50
  byte 1  = 4
  bytes 2-5 = 0x30, 0x48, 0x60, 0x6C

IE DS Parameter Set (3): current channel
  byte 0  = 3
  byte 1  = 1
  byte 2  = <channel>
```

Total min ~ 24 + 2 + 10 + 6 + 3 = **45 Bytes** für eine Wildcard-Probe.

---

## 10 — Was aktuell im Code fehlt

### TX-Ring-Setup (init-time)

- [ ] **TXBD-Ring** für CH8 in DMA-coherent alloc (256×8 = 2KB)
  - Write `_TXBD_DESA_L/H` (0x1150/0x1154)
  - Write `_TXBD_NUM` (Ringgrösse)
  - Write `_BDRAM` (sidx=20, max=4, min=1)
  - Reset `wp = rp = 0` via `TXBD_RWPTR_CLR1` bit `CLR_CH8_IDX`=bit8
- [ ] **TXWD-Ring** für CH8 in DMA-coherent alloc (512×128 = 64KB)
  - Liste `free_pages` aufbauen (alle 512 Pages initial frei)

### TX-Write-Path

- [ ] `tx_write_mgmt(frame_bytes)`:
  - Page vom free-list nehmen
  - Frame-DMA mappen
  - TXWD-Body/Info + TXWP + Addr-Info in Page schreiben (siehe Kap 3-5)
  - TXBD im BD-Ring schreiben
  - `wp++`
  - write16(0x1078, wp) — Kick-Off

### TX-Completion

- [ ] Polling auf TX-Completion via BUSY-Register (0x101C bit 16 = CH8_BUSY)
- [ ] oder IRQ-driven: TX-completion C2H-event
- [ ] oder: einfach Timeout + auf RX warten (für Probe Req → Probe Resp)

### Frame-Builder

- [ ] `build_probe_request(sa: [u8;6], channel: u8, buf: &mut [u8]) -> usize`

### Integration in Init

- [ ] Nach `pcie_dma_pre_init` auch CH8-Ring aufsetzen
- [ ] DMA-Stop-Clear prüfen (ist OK)

---

## 11 — Phase 1 Minimum (Probe Request Smoke-Test)

**Sequenz:**

1. Init-Time (nach VIF-Init):
   - CH8 TXBD-Ring allozieren (z. B. 32 BDs für Start)
   - CH8 TXWD-Ring allozieren (z. B. 16 Pages à 128 B = 2 KB)
   - DESA_L/H + BDRAM + NUM schreiben
   - DMA bereits enabled

2. Runtime:
   - Probe Request Frame builden (ch 7 für IvyPie)
   - `tx_write_mgmt(frame)` → Page + BD + Kick
   - RX-Loop pollen: auf Probe Response von `b4:fc:7d:56:a2:e8` warten

**Erfolgs-Indikator**: Probe Response empfangen innerhalb 200 ms.

**Fallback**: Wenn Probe Resp nicht kommt:
- Check `R_AX_PCIE_DMA_BUSY1 (0x101C)` bit 16 → CH8_BUSY bewegt?
- Check `R_AX_CH8_TXBD_IDX (0x1078)` → `hw_idx` gestiegen?
- Frame auf Luft: anderer Sniffer (z. B. aircrack auf einem Nachbar-Gerät) → sehen wir den Frame?

---

## Offene Punkte (ausserhalb Phase 1, später)

- TX-completion-Reclaim: `rtw89_pci_release_tx`, RPP-fmt parsen
- `tx_kick_off_and_wait`: Completion-Mechanismus für blocking sends
- `hiq` Queue (CH9) für high-priority Mgmt
- Band-1 Mgmt (CH10) — für 8852BE nicht nötig (single-band)
- BE/V1-Chips (TXD v1/v2/v3, andere Register-Layouts)

---

## 12 — FW-Scan vs Direct-TX (Konfliktanalyse)

### Wie Linux Scan macht (`fw.c:8280 rtw89_hw_scan_update_probe_req`)

1. User startet scan → `ieee80211_probereq_get` baut Frame pro SSID
2. `rtw89_fw_h2c_add_pkt_offload` lädt Frames in FW-internen Paket-Pool (gibt ID zurück)
3. `H2C_FUNC_SCAN_OFFLOAD` startet FW-gesteuerten Scan
4. FW wechselt Kanäle selbst, sendet registrierte Probe-Req-Pakete
5. Beacons/Probe-Resp kommen zurück via RX-Queue

→ **FW macht Scan autonom**. Unsere Direct-TX ist dabei NICHT aktiv.

### Wie Linux AUTH/ASSOC macht

`mac80211.c:19 rtw89_ops_tx` ist der **einzige** TX-Eintrittspunkt für
AUTH/ASSOC/Data. Kein spezieller `mgd_prepare_tx`-Callback (grep leer).

Pfad:
```
mac80211 → rtw89_ops_tx → rtw89_core_tx_write → rtw89_pci_ops_tx_write
                                             → CH8 TXBD-Ring + Kick-Off
```

→ **AUTH/ASSOC ist Direct-TX über CH8** — genau das was wir bauen.

### Konflikt-Befund

- **Während scan**: FW sendet via internem Pool. Wir schicken nichts.
- **Nach scan**: FW ruhig. Direct-TX auf CH8 frei.
- **Pre-TX Setup**: Kein besonderer Hook nötig — VIF-Init (haben wir) genügt.

**Kritisch vor AUTH**:
- `set_channel(target_ap_channel)` (wir haben als `chan::set_channel`)
- VIF muss CREATED + macid_pause=unpause sein (✓, VIF-Init macht das)

### Etappe 1 Strategie (final)

1. `driver wifi` → volles Init inkl. 3-pass scan (wie heute)
2. **NACH** scan: `set_channel(7)` (IvyPie)
3. Probe Req Direct-TX auf CH8 (gerade-aufgesetzter Ring)
4. RX-Poll 200ms auf Probe Response mit BSSID `b4:fc:7d:56:a2:e8`
5. Bei Erfolg: TX funktioniert. → Etappe 2 (AUTH).

---

## 13 — Probe Request ohne mac80211 (Frame-Builder)

Minimaler Wildcard Probe Request (für unseren Smoke-Test):

```
24 Bytes MAC Header:
  fc[0]=0x40 fc[1]=0x00   (Type=0 Mgmt, Subtype=4 ProbeReq)
  dur[2..3]=0x0000
  da[4..9]  = FF:FF:FF:FF:FF:FF
  sa[10..15]= <unsere MAC, 6 B>
  bssid[16..21] = FF:FF:FF:FF:FF:FF
  seq[22..23] = 0x0000  (HW füllt via HW_SSN_SEL=1)

Body (Wildcard Probe):
  0x00, 0x00                              (IE SSID tag=0, len=0)
  0x01, 0x08, 0x82,0x84,0x8B,0x96,
        0x0C,0x12,0x18,0x24               (IE Rates, 8 basic rates)
  0x32, 0x04, 0x30,0x48,0x60,0x6C         (IE Ext-Rates, 4 rates)
  0x03, 0x01, <channel>                   (IE DS Param)

Total: 24 + 2 + 10 + 6 + 3 = 45 Bytes
```

Frame-Adressen:
- **DA/BSSID Broadcast** → der AP akzeptiert, antwortet wenn SSID leer (Wildcard)
- **SA** unsere MAC (aus PCI-Config oder Efuse — haben wir?)

Prüfen: `SA` sollte unsere MAC sein (aus VIF-Init). Falls unklar, testweise
mit gefakten 02:00:00:00:00:01 — verhindert aber spätere Assoc.
