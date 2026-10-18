# AMD-Vi am IdeaPad: die Karte nur auf freigegebene Seiten

Status: Papier, nichts gebaut. Ziel: dieselbe Rolle wie `drivers/vtd.rs` am
NUC, fuer die IdeaPad-GPU (Ryzen 5 5500U, Zen 2 Lucienne, `1002:164c` an
`04:00.0`), spaeter fuer weitere Geraete. Quelle: Linux 6.18.55,
`drivers/iommu/amd/` (alle Zeilennummern beziehen sich darauf; `init.c`,
`iommu.c`, `io_pgtable.c`, `amd_iommu_types.h` = `types.h`).

Regel wie immer: die Linux-Funktionen auf dem Weg ganz portieren, nicht
nachbauen. Was wir bewusst weglassen (kdump, SNP, PPR, GT/v2, IR), steht
unten mit Grund.

---

## 1 — Der Vertrag: was VT-d dem Kernel heute gibt

`kernel/src/drivers/vtd.rs` (637 Zeilen), oeffentlich:

| Funktion | Bedeutung |
|---|---|
| `protect(dev) -> Result<(), &str>` (400) | Geraet hinter seine Einheit stellen, IOVA = phys 1:1; danach erreicht es nur RMRR, Stolen Memory und was `grant` dazulegt. Bleibt an bis zum Neustart. |
| `protected(dev) -> bool` (506) | ist genau dieses Geraet eingesperrt |
| `grant(dev, phys, pages)` / `grant_list(dev, &[u64])` (561/512) | Seiten freigeben, **referenzgezaehlt** je Seite; nicht-vorhanden → vorhanden braucht keinen Flush (ausser Caching Mode) |
| `revoke(dev, phys, pages)` / `revoke_list` (588/539) | Freigabe zuruecknehmen; **wenn es zurueckkehrt, erreicht das Geraet die Seite nicht mehr** (IOTLB-Flush synchron) |
| `poll(dev)`, `take_faults(dev) -> Vec<Fault>` (610/618) | Fehlerprotokoll pollen (keine Unterbrechung); `Fault { addr, source, reason, read }` |
| `status() -> Option<Status>` (632) | `dev`, Seitenzahl, Fehler gesamt — fuer `gpu iommu` |

Alle Aufrufe sind fuer ein ungeschuetztes Geraet No-ops — darauf verlassen
sich die Rufer (sie rufen `grant`/`revoke` fuer jedes PCI-Geraet).

Rufstellen:

- `kernel/src/wasm/host_core.rs:1539` — `npk_pci_enable_bus_master`: ist das
  Geraet Klasse 0x03 und `crate::gpu::iommu_wanted()`, dann `protect` und alle
  schon angelegten `dma_allocs` freigeben, **bevor** Bus-Mastering angeht.
- `host_core.rs:2083`, `:2127` — `npk_dma_alloc*`: `grant`; `:2054`
  `npk_dma_free`: erst `revoke`, dann Speicher zurueck.
- `kernel/src/wasm.rs:1128-1142` — Modulende: eingesperrte Karte → `revoke`
  und Speicher darf zurueck, auch waehrend die Anzeige noch scannt.
- `kernel/src/gpu/gpudev.rs:138-147` — `Grant::apply` (Bereich oder fremde
  Seitenliste); `:588` `poll`; `:1249` `native_device()` verlangt
  `vtd::protected(owner)` — **ohne das keine native contexts fuer Gaeste**.
- `kernel/src/shade/gpu_scene.rs:577-660` — `iommu_probe` (`gpu iommu test`):
  Szene mit freigegebener Quelle → Muster kommt an, 0 Fehler; dann Quelle
  `revoke`, gleiche Szene → Muster kommt **nicht** an, Fehler mit Adresse
  **in** der Quelle aufgezeichnet; danach `grant` zurueck.
- `kernel/src/intent/system.rs:1204` — `gpu iommu`: Status + Fehlerliste.
- Schalter: `set gpu_iommu on` (`kernel/src/gpu/mod.rs:151`, Vorgabe aus).

**Folgerung fuer AMD-Vi:** dieselben acht Funktionen mit derselben Semantik.
Die Rufer sollen den Hersteller nicht kennen: ein `drivers::iommu` als
Weiche (`vtd` oder `amd_vi`, je nachdem welche Tabelle da ist — DMAR oder
IVRS), die Rufer werden einmal umbenannt, `gpu iommu` sagt „behind AMD-Vi".

**Der eine echte Unterschied:** `vtd::protect` nimmt nur eine Einheit, deren
Bereich *allein* dieses Geraet ist (`vtd.rs:410-413`), damit das Einschalten
kein anderes Geraet beruehrt. Auf Renoir gibt es **eine** IOMMU fuer alle
Geraete (IVHD mit `DEV_ALL`/Bereichen). Die Trennung leistet hier der
**Geraetetabelleneintrag (DTE) je Requester-ID** — siehe §2.9: ein DTE mit
`V=0` laesst durch, also kann die IOMMU eingeschaltet werden, ohne dass sich
fuer NVMe, xHCI, Audio irgendetwas aendert.

---

## 2 — Der Linux-Weg fuer eine IOMMU und ein Geraet

Linux-Ablauf: `state_next` (init.c:3478) — `detect_ivrs` (3366) →
`early_amd_iommu_init` (3232) → `early_enable_iommus` (2925) →
Ereignispuffer + `amd_iommu_init_pci` (2319) → Geraete bekommen ihre Domaene
ueber den IOMMU-Kern (`attach_device`, iommu.c:2287).

### 2.1 IVRS lesen

Unser Zugang: `crate::drivers::acpi::find_table(b"IVRS")` /
`find_table_nth`, `ensure_mapped_pub` (wie `vtd.rs` `parse` mit DMAR);
`acpi::platform_mmio_ranges` (acpi.rs:166) reserviert die IVHD-Registerbloecke
schon mit 512 KiB.

- Kopf 48 Bytes (`IVRS_HEADER_LENGTH`, init.c:43), Pruefsumme
  `check_ivrs_checksum` (585), IVinfo bei Offset 36 (`ivinfo_init` 3200;
  `IOMMU_IVINFO_EFRSUP` / `_DMA_REMAP`, types.h:462-464).
- `get_highest_supported_ivhd_type` (2011): **nur ein IVHD-Typ wird benutzt**,
  der hoechste ≤ 0x40 fuer dieselbe IOMMU-devid. Firmware listet dieselbe
  IOMMU oft als 10h **und** 11h/40h — beide zu parsen hiesse sie doppelt
  anzulegen.
- `struct ivhd_header` (101): type, flags, length, devid (die IOMMU selbst,
  auf Renoir 00:00.2), cap_ptr, mmio_phys, pci_seg, info, efr_attr; bei
  11h/40h zusaetzlich `efr_reg`/`efr_reg2` (exakte Kopie von EFR/EFR2).
  Kopfgroesse `get_ivhd_header_size` (499): 10h = 24, 11h/40h = 40.
- Eintraege (`struct ivhd_entry`, 121), Laenge `ivhd_entry_length` (527):
  Typ < 0x80 → `4 << (type >> 6)`, `0xF0` (ACPI_HID) → `22 + uid_len`.
- Zwei Durchgaenge wie Linux: `find_last_devid_acpi` (606) /
  `find_last_devid_from_ivhd` (544) bestimmt `last_bdf` (bei `DEV_ALL` 0xffff),
  dann `alloc_pci_segment` (1723): Geraetetabelle
  `max(roundup_pow_of_two((last_bdf+1)*32), 4K)`, Alias-Tabelle (697),
  `alloc_dev_table` (643, `GFP_DMA32`).
- `init_iommu_from_acpi` (1461) durchlaeuft die Eintraege
  (`IVHD_DEV_*`, init.c:50-59): `ALL` (1505), `SELECT` (1510),
  `SELECT_RANGE_START` … `RANGE_END` (1521/1593), `ALIAS` (1534) und
  `ALIAS_RANGE` (1551) fuellen `alias_table[devid] = devid_to`,
  `EXT_SELECT[_RANGE]` (1568/1580, `ext` traegt `ACPI_DEVFLAG_ATSDIS`),
  `SPECIAL` (1608, IOAPIC/HPET — nur fuer IR noetig), `ACPI_HID` (1645).
  Die Flags gehen ueber `set_dev_entry_from_acpi_range` (1289) als
  „persistente" DTE-Bits in eine Liste (INITPASS/EINT/NMI/SYSMGT/LINT0/1,
  init.c:78-85) und in die Tabelle; `set_dte_entry` ODERt sie spaeter wieder
  dazu (iommu.c:2124-2128, `amd_iommu_get_ivhd_dte_flags` init.c:1247).
- IVMD (`struct ivmd_header` 142; Typen 20h ALL, 21h ein Geraet, 22h Bereich,
  init.c:46-48): `init_memory_definitions` (2708) → `init_unity_map_range`
  (2645). Flags `UNITY_MAP`/`IR`/`IW`/`EXCL_RANGE` (73-76); ein
  Ausschlussbereich wird als r/w-Unity-Map behandelt (2690). Linux legt diese
  Bereiche als `IOMMU_RESV_DIRECT` an (`amd_iommu_get_resv_regions`,
  iommu.c:2932) und der IOMMU-Kern mappt sie 1:1 in die Domaene
  (`iommu_create_device_direct_mappings`, drivers/iommu/iommu.c:1157).
  **Das ist das Gegenstueck zu RMRR** — `protect` muss jedes IVMD, das die
  GPU-devid abdeckt, als Grundbestand freigeben.

### 2.2 IOMMU anlegen und MMIO abbilden

`init_iommu_all` (2035) → `init_iommu_one` (1891): devid, cap_ptr, mmio_phys
uebernehmen; Fenstergroesse `MMIO_CNTR_CONF_OFFSET` (0x4000) oder, wenn
Leistungszaehler gemeldet sind (`efr_reg` Bit 9 bei 11h/40h, 1936),
`MMIO_REG_END_OFFSET` (0x80000); `HATDIS` pruefen (1941);
`early_iommu_features_init` (305) nimmt EFR aus dem IVHD, wenn IVinfo
`EFRSUP` sagt. `iommu_map_mmio_space` (480) = bei uns `Mmio::map`.
Danach `get_global_efr` (268) und `init_iommu_one_late` (1970):
`alloc_iommu_buffers` (1059: `alloc_cwwb_sem` 1001 = eine 4K-Seite fuer den
Completion-Wait-Semaphor, `alloc_command_buffer` 747 = 8 KiB),
`init_translation_status` (245) — **war die IOMMU schon an, wird sie
abgeschaltet** (1981) — und: die IOMMU uebersetzt sich nicht selbst
(1998, „The IVRS table tells us so, but this is a lie!").

`early_amd_iommu_init` dazu: CMPXCHG16B Pflicht (3251), Host-Tabellentiefe
aus `EFR.HATS` (`amd_iommu_hpt_level = HATS + 4`, sonst Uebersetzung aus),
Vorgabe `amd_iommu_pgtable = PD_MODE_V1` (init.c:157), `disable_iommus`
(3053) vor allem anderen.

Register (types.h:56-84): `DEV_TABLE 0x00`, `CMD_BUF 0x08`, `EVT_BUF 0x10`,
`CONTROL 0x18`, `EXCL_BASE/LIMIT 0x20/0x28`, `EXT_FEATURES 0x30`,
`CMD_HEAD/TAIL 0x2000/0x2008`, `EVT_HEAD/TAIL 0x2010/0x2018`,
`STATUS 0x2020`. Control-Bits (163-189): `IOMMU_EN 0`, `HT_TUN_EN 1`,
`EVT_LOG_EN 2`, `EVT_INT_EN 3`, `COMWAIT_EN 4`, `INV_TIMEOUT 5..7`,
`PASSPW 8`, `RESPASSPW 9`, `COHERENT 10`, `ISOC 11`, `CMDBUF_EN 12`,
`GT_EN 16`, `GA_EN 17`, `XT_EN 50`.

### 2.3 Einschalten (`early_enable_iommu`, init.c:2901)

In genau dieser Reihenfolge:

1. `iommu_disable` (449) — CMDBUF, EVT, GALOG, PPR, IOMMU_EN aus.
2. `iommu_init_flags` (2773) — HT_TUN/PASSPW/RESPASSPW/ISOC **aus den
   IVHD-Flags** (init.c:68-71), `COHERENT_EN` an, IOTLB-Timeout 1 s
   (`CTRL_INV_TO_1S`, types.h:196), EPH nur mit `FEATURE_EPHSUP`.
3. `iommu_set_device_table` (404) — `phys | (size >> 12) - 1`.
4. `iommu_enable_command_buffer` (821) — `phys | MMIO_CMD_SIZE_512`
   (types.h:249), dann `amd_iommu_reset_cmd_buffer` (805): Head/Tail 0,
   `CMDBUF_EN`.
5. `iommu_set_exclusion_range` (362) — nur wenn ein IVHD/IVMD einen
   Ausschlussbereich gesetzt hat.
6. `iommu_enable_gt` (1139), `iommu_enable_ga` (2847), `iommu_enable_xt`
   (1126), `iommu_enable_irtcachedis` (2868), `iommu_enable_2k_int` (2890) —
   alle haengen an Merkmalen bzw. an IR; siehe §2.8/§3.
7. `iommu_enable` (444) — `IOMMU_EN`.
8. `amd_iommu_flush_all_caches` (iommu.c:1547) — mit `FEATURE_IA`
   `INVALIDATE_ALL` (1514), sonst alle DTEs, IRTs, TLBs einzeln.

Danach (state `IOMMU_ENABLED`, 3505-3523): `alloc_event_buffer` (874, 8 KiB =
512 Eintraege) und `iommu_enable_event_buffer` (888): `phys |
EVTLOG_LEN_MASK_DEF` (types.h:255), Head/Tail 0, `EVT_LOG_EN`. Dann
`amd_iommu_init_pci` (2319) → `iommu_init_pci` (2171): Capability-Kopf
lesen (`IOMMU_CAP_EFR`/`IOTLB`/`NPCACHE`, types.h:457-459),
`late_iommu_features_init` (2141) vergleicht EFR aus MMIO mit IVHD,
Errata (§3), `pci_enable_device`. Zuletzt `init_device_table_dma` (2730)
und nochmals `amd_iommu_flush_all_caches`.

### 2.4 Befehlspuffer, Invalidierung, Abschluss

16-Byte-Befehle, Typ in `data[1] << 28` (`CMD_SET_TYPE`, iommu.c:50;
Typen types.h:201-207).

- `__iommu_queue_command_sync` (1338): Platz pruefen (≥ 0x20 frei, sonst
  `CMD_HEAD` nachlesen, `LOOP_TIMEOUT` 100000 × 1 µs, types.h:480),
  `copy_cmd_to_buffer` (1182): kopieren, Tail weiter, `CMD_TAIL` schreiben.
- `build_inv_dte` (1214): `INVALIDATE_DEVTAB_ENTRY`, devid in `data[0]`.
- `build_inv_iommu_pages` (1263) mit `build_inv_address` (1225): Domain-ID in
  `data[1]`, Adresse in `data[2..3]`, **PDE-Bit immer** (auch Verzeichnisse
  flushen), S-Bit fuer mehr als eine Seite — der Bereich wird auf die
  kleinste natuerlich ausgerichtete Zweierpotenz aufgerundet, die ihn
  abdeckt; `CMD_INV_IOMMU_ALL_PAGES_ADDRESS` (types.h:218) fuer alles.
- `build_completion_wait` (1200): Speicheradresse des Semaphors + `STORE`,
  64-Bit-Wert; `iommu_completion_wait` (1404) zieht `++cmd_sem_val` und
  `wait_on_sem` (1160) pollt den Speicher, bis der Wert erreicht ist.
  **Das ist der Punkt, an dem `revoke` zurueckkehren darf.**
- Zusammengesetzt: `iommu_flush_dte_sync` (1464), `amd_iommu_flush_tlb_domid`
  (1503), `amd_iommu_domain_flush_pages` (1697) → `__domain_flush_pages`
  (1669) → `domain_flush_pages_v1` (1641) → `domain_flush_complete` (1440).

### 2.5 Ereignisprotokoll

`iommu_poll_events` (984): Head/Tail lesen, je 16 Bytes
(`EVTLOG_ENTRY_SIZE`, types.h:252) `iommu_print_event` (886), Head weiter.
Felder (types.h:140-160): Typ `event[1] >> 28`, devid `event[0] & 0xffff`,
Flags `(event[1] >> 16) & 0xfff`, Adresse `event[3]:event[2]`.
`IO_PAGE_FAULT` = 2 (`amd_iommu_report_page_fault`, 835), RW-Flag 0x20 =
Schreiben, I-Flag 0x08 = Interrupt- statt Speicherzugriff (829-833). Typ 0
heisst „Eintrag noch nicht geschrieben" → bis zu `LOOP_TIMEOUT` warten
(904), danach den Eintrag nullen (Erratum 732, 981). Abbildung auf unser
`Fault`: `addr`, `source = devid`, `reason = Typ`, `read = flags & 0x20 == 0`.
Wie bei VT-d gepollt, ohne MSI (`EVT_INT_EN` bleibt aus, Linux richtet es
ueber `iommu_init_irq` 2595 ein — benannte Abweichung, wie `vtd.rs` Kopf).

### 2.6 Seitentabelle v1 (`io_pgtable.c`)

- `v1_alloc_pgtable` (551): Wurzel eine 4K-Seite, **Start mit
  `PAGE_MODE_3_LEVEL`** (39 Bit = 512 GB).
- Bits (types.h:345-414): Index je Ebene `PM_LEVEL_INDEX` = 9 Bit ab
  `12 + 9·lvl`; Eintrag: `PR` Bit 0, naechste Ebene in Bit 9-11
  (`PM_LEVEL_ENC`), Adresse Bit 12-51 (`PM_ADDR_MASK`), `FC` 60,
  `IR` 61, `IW` 62. Verzeichnis = `PM_LEVEL_PDE`: Adresse | Ebene | PR | IR |
  IW. Blatt 4K: `paddr | PR | FC | IR | IW` (`iommu_v1_map_pages` 337, 374).
  Ebene 7 = grosse Seite, brauchen wir nicht (wir mappen 4K).
- `alloc_pte` (151) laeuft hinunter und legt fehlende Verzeichnisse an;
  reicht die Tiefe nicht, `increase_address_space` (106): neue Wurzel ueber
  die alte, `mode += 1`, `amd_iommu_update_and_flush_device_table`
  (iommu.c:1797) — der DTE traegt Wurzel und Tiefe.
- `iommu_v1_map_pages` (337): **geflusht wird nur, wenn ein vorhandener
  Eintrag ersetzt wurde** (`updated`, 393-405) — nicht-vorhanden → vorhanden
  braucht keinen Flush. Ausnahme `amd_iommu_np_cache` (Capability NPCACHE,
  init.c:2215; nur in VMs): dann `domain_flush_np_cache` nach jedem Map
  (iommu.c:1781). Deckt sich mit `vtd::grant` + Caching Mode.
- `iommu_v1_unmap_pages` (414) nullt die Eintraege; der Flush kommt ueber
  `amd_iommu_iotlb_gather_add_page` (2794) und `amd_iommu_iotlb_sync`
  (3011) — **ein** Flush fuer den gesammelten Bereich, wie `revoke_list`.
- `fetch_pte` (258) fuer Unmap; leere Verzeichnisse gibt Linux beim Unmap
  NICHT frei (erst `free_sub_pt` beim Domaenenende) — wir auch nicht.

### 2.7 Ein Geraet an die Domaene haengen

`protection_domain_alloc` (iommu.c:2502, Domain-ID ≠ 0) →
`pdom_setup_pgtable` (2523) → `attach_device` (2287) →
`pdom_attach_iommu` (2223) → `dev_update_dte(dev_data, true)` (2157):

- `set_dte_entry` (2074): `make_clear_dte` (2030) = nur `V`; dann
  `data[0] |= root_phys | (mode & 7) << 9 | IR | IW | TV` (2092-2106),
  `data[1] |= domid` (2118), IVHD-Bits dazu (2124), dann `update_dte256`.
- `update_dte256` (134): der DTE ist 256 Bit und wird in zwei 128-Bit-
  `cmpxchg16b` geschrieben (`write_dte_upper128` 102 — behaelt die
  Interrupt-Bits `DTE_DATA2_INTR_MASK` —, `write_dte_lower128` 118); **V
  zuletzt beim Setzen, zuerst beim Loeschen**; danach `iommu_flush_dte_sync`.
- `clone_aliases` (427) schreibt denselben DTE auf jede DMA-Alias-devid,
  `device_flush_dte` (1584) flusht devid, PCI-Aliase und `alias_table`-Alias.
- **ATS:** Linux schaltet ATS fuer ein faehiges Geraet ein
  (`pdev_enable_cap_ats`, 2329) und setzt `DTE_FLAG_IOTLB`. Fuer uns NICHT:
  mit ATS haelt die Karte eigene Uebersetzungen und darf vorab uebersetzte
  Adressen schicken — genau das, was wir einem Modul nicht glauben. Linux
  sagt dasselbe fuer `untrusted`-Geraete (`amd_iommu_def_domain_type`,
  3032; `pci_ats_supported` lehnt sie ab). Unser DTE: `IOTLB = 0`.

### 2.8 Interrupt-Remapping — brauchen wir es?

Linux: `amd_iommu_prepare` (init.c:3599) setzt `amd_iommu_irq_remap = true`,
IR ist auf x86 also Vorgabe (sofern `check_ioapic_information` 3129 die
IOAPICs im IVHD findet). Dann bekommt **jeder** DTE `IV`
(`init_device_table`, 2759) und jedes MSI geht durch eine IRTE.

Fuer Stufe 1-3 **nein**: ohne IR bleibt `IV = 0`, und Interrupts (Schreiben
in 0xFEEx_xxxx) laufen unuebersetzt durch, fuer alle Geraete wie heute. Die
DMA-Uebersetzung haengt nicht daran (Linux betreibt beides getrennt: die
Fehlerbehandlung in `state_next` 3546-3561 laesst bei kaputter
DMA-Uebersetzung IR an und setzt die DTEs auf V=TV=0). `vtd.rs` behaelt IR
ebenfalls nur so, wie es vorgefunden wurde.

**Aber benannt:** ohne IR kann eine eingesperrte Karte beliebige
MSI-Nachrichten schreiben, also beliebige Vektoren auf beliebigen Kernen
ausloesen. Das ist keine Speicherflucht, aber eine Stoerung von aussen —
dieselbe Luecke besteht heute am NUC. IR ist ein eigener Posten (Stufe 5),
fuer beide Hersteller.

### 2.9 Geraete OHNE Domaene

- **DTE `V = 0`: Durchgang ohne Uebersetzung.** Linux nutzt das selbst: nach
  `early_enable_iommu` ist die IOMMU an und alle DTEs ohne V, bis
  `init_device_table_dma` (2730) spaeter V+TV setzt. Und der Fehlerpfad
  `uninit_device_table_dma` (2745) nullt die DTEs gerade, damit DMA weiter
  geht („DTE[V, TV] must eventually be set to 0", 2275).
- **Blockiert:** `init_device_table_dma` setzt fuer jede devid `V | TV`
  ohne IR/IW, Mode 0 — „not allow DMA access" (2728). (6.18 laesst in
  `make_clear_dte` 2030 fuer die blockierte Domaene TV weg; vor dem Bau
  gegen die AMD-IOMMU-Spezifikation pruefen, welche Form wir nehmen.)
- **Identitaet:** Mode 0 (`PAGE_MODE_NONE`) mit `V | TV | IR | IW`
  (`set_dte_entry` ohne Wurzel; `identity_domain` 2695).

Unser Plan: alle DTEs bleiben `V = 0` (Durchgang wie bisher), nur die
GPU-devid bekommt einen uebersetzten DTE. Das ist das AMD-Gegenstueck zu
„Einheit nur fuer dieses Geraet": es aendert sich fuer niemanden sonst etwas.
Linux-Vorgabe ist das Gegenteil (alles blockiert, jedes Geraet bekommt
DMA- oder Identitaetsdomaene) — das waere Stufe 6.

### 2.10 Zu portierende Funktionen, in Reihenfolge

| # | Funktion | Datei:Zeile |
|---|---|---|
| 1 | `check_ivrs_checksum`, `ivinfo_init`, `get_highest_supported_ivhd_type` | init.c:585, 3200, 2011 |
| 2 | `get_ivhd_header_size`, `ivhd_entry_length`, `find_last_devid_from_ivhd`, `find_last_devid_acpi` | init.c:499, 527, 544, 606 |
| 3 | `alloc_pci_segment` (+ `alloc_dev_table`, `alloc_alias_table`) | init.c:1723, 643, 697 |
| 4 | `init_iommu_one`, `early_iommu_features_init`, `iommu_map_mmio_space` | init.c:1891, 305, 480 |
| 5 | `init_iommu_from_acpi`, `set_dev_entry_from_acpi_range`, `set_dte_bit` | init.c:1461, 1289, 1148 |
| 6 | `get_global_efr`, `init_iommu_one_late`, `alloc_iommu_buffers`, `init_translation_status` | init.c:268, 1970, 1059, 245 |
| 7 | `init_memory_definitions`, `init_unity_map_range` | init.c:2708, 2645 |
| 8 | `iommu_feature_set/enable/disable`, `iommu_disable`, `disable_iommus` | init.c:421-447, 449, 3053 |
| 9 | `early_enable_iommu` mit `iommu_init_flags`, `iommu_set_device_table`, `iommu_enable_command_buffer`, `amd_iommu_reset_cmd_buffer`, `iommu_set_exclusion_range`, `iommu_enable` | init.c:2901, 2773, 404, 821, 805, 362, 444 |
| 10 | `__iommu_queue_command_sync`, `copy_cmd_to_buffer`, `build_completion_wait`, `iommu_completion_wait`, `wait_on_sem` | iommu.c:1338, 1182, 1200, 1404, 1160 |
| 11 | `build_inv_dte`, `build_inv_address`, `build_inv_iommu_pages`, `build_inv_all`, `amd_iommu_flush_all_caches` (+ `_dte_all`, `_tlb_all`) | iommu.c:1214, 1225, 1263, 1321, 1547 |
| 12 | `alloc_event_buffer`, `iommu_enable_event_buffer`, `iommu_poll_events`, `iommu_print_event` | init.c:874, 888; iommu.c:984, 886 |
| 13 | `iommu_init_pci` (Cap-Kopf), `late_iommu_features_init` | init.c:2171, 2141 |
| 14 | `v1_alloc_pgtable`, `alloc_pte`, `increase_address_space`, `fetch_pte`, `iommu_v1_map_pages`, `iommu_v1_unmap_pages` | io_pgtable.c:551, 151, 106, 258, 337, 414 |
| 15 | `set_dte_entry`, `make_clear_dte`, `update_dte256`, `write_dte_upper/lower128`, `clone_aliases`, `device_flush_dte`, `dev_update_dte` | iommu.c:2074, 2030, 134, 102/118, 427, 1584, 2157 |
| 16 | `amd_iommu_domain_flush_pages`, `__domain_flush_pages`, `domain_flush_pages_v1`, `domain_flush_complete`, `amd_iommu_update_and_flush_device_table` | iommu.c:1697, 1669, 1641, 1440, 1797 |
| 17 | `amd_iommu_get_resv_regions` (nur der IVMD-Teil, als Grundfreigabe) | iommu.c:2932 |

Nicht portiert, mit Grund: kdump-Wiederverwendung (`reuse_device_table`
1220), SNP (`iommu_snp_enable` 3404, CWWB 380), PPR/IOPF (`ppr.c`), GCR3/v2
(`io_pgtable_v2.c`, `set_dte_gcr3_table` 2042), GA/vAPIC, IR (§2.8),
Leistungszaehler, sysfs, Suspend.

---

## 3 — Zen 2 / Renoir: was die Quelle dazu sagt

- **Keine Renoir-Errata im Treiber.** `amd_iommu_erratum_746_workaround`
  (init.c:1834) und `amd_iommu_ats_write_check_workaround` (1865) greifen nur
  bei Familie 0x15; Zen 2 ist 0x17. Portieren (1:1, mit ihrer Familienpruefung)
  kostet nichts und tut am IdeaPad nichts.
- **Erratum 63** (init.c:1323): SysMgt = 01b in den IVHD-Flags → `IW` im DTE.
  Steckt in `set_dev_entry_from_acpi_range`, kommt also mit.
- **Erratum 732** (iommu.c:972): Ereigniseintrag nach dem Lesen nullen.
- **Stoney Ridge** (`detect_ivrs`, 3386) schaltet die IOMMU ab — nur
  `1002:98e4`, nicht unsere Karte.
- **DMI-Quirks** (`quirks.c`): nur IOAPIC-Zuordnungen fuer Dell/Acer/Lenovo
  IdeaPad 330S — betrifft nur IR, nicht uns.
- **IVHD-Typ:** Renoir-Firmware liefert ueblicherweise 10h und 11h (oft auch
  40h) fuer dieselbe IOMMU; wir nehmen per §2.1 den hoechsten. EFR kommt dann
  aus dem IVHD (`efr_reg`), `late_iommu_features_init` warnt bei Abweichung.
- **EFR-Merkmale, die wir lesen, aber nicht einschalten:** `GT` (Bit 4,
  Gast-Uebersetzung/PASID), `PPR` (Bit 1), `GA`/`GAM_VAPIC`, `XT`. Linux
  setzt `GT_EN` sobald das Merkmal da ist (`iommu_enable_gt`, 1139) — ohne
  GCR3-Tabelle (`DTE.GV = 0`) ist es wirkungslos; der Port uebernimmt den
  Aufruf trotzdem 1:1. `IA` (Bit 6) entscheidet `INVALIDATE_ALL`, `HATS`
  (Bit 10-11) die maximale Tiefe (4 + HATS).
- **Linux gibt genau dieser Karte standardmaessig KEINE Uebersetzung:**
  `amd_iommu_def_domain_type` (iommu.c:3023) gibt einem PASID-faehigen
  Geraet (AMD-GPUs) die IDENTITAETSdomaene, ausser bei Speicherverschluesselung
  oder SNP. amdgpu merkt sich das als `ram_is_direct_mapped`
  (amdgpu_device.c:4429). Wir weichen bewusst ab — der Zweck ist die
  Einsperrung. Folge fuer `gpu_amd_renoir`: es darf `ram_is_direct_mapped`
  nicht annehmen; IOVA = phys bleibt trotzdem wahr, weil wir 1:1 mappen.
- **APU-Besonderheit (zu messen, nicht aus der Quelle):** der VRAM-Ausschnitt
  (UMA-Carve-out, BAR 0) wird vom GMC der Karte ueber den Data Fabric
  gelesen, nicht als PCIe-DMA — erwartet ist, dass Scanout aus dem
  Carve-out die IOMMU nie sieht. Systemspeicher ueber GART/GPUVM (GTT) geht
  durch sie. Stufe 2 beweist oder widerlegt das.
- **Requester-IDs am IdeaPad:** auf Bus 04 liegen GPU (04:00.0),
  HDMI-Audio (.1), zwei xHCI (.3/.4), FCH-Audio (.6), Sensor Fusion Hub (.7)
  hinter derselben Bruecke — jede Funktion hat ihren DTE; ein uebersetzter
  DTE fuer .0 beruehrt die anderen nicht, solange kein IVHD-ALIAS sie auf
  eine devid faltet (Stufe 0 druckt die Alias-Tabelle).

---

## 4 — Stufen

Jede Stufe mit sichtbarem Ergebnis am IdeaPad; Florian startet die Laeufe.

**Stufe 0 — nur lesen.** IVRS parsen (§2.10 #1-7), MMIO abbilden, Control,
Status, EFR aus MMIO lesen. Nichts schreiben. Neue Zeile in `gpu iommu`
(oder Bootlog unter `kdebug!`): IVHD-Typ, devid der IOMMU, MMIO-Basis,
`last_bdf`, EFR/EFR2 entziffert (GT IA GA HATS …), Control (muss `IOMMU_EN=0`
zeigen), IVMD-Liste, Alias-Eintraege fuer Bus 04, persistente DTE-Flags.
*Test:* Ausgabe gegen `dmesg | grep AMD-Vi` eines Linux-Live-Sticks am selben
Geraet (`amd_iommu_dump` gibt dieselben Felder aus).

**Stufe 1 — einschalten, alles Durchgang.** Geraetetabelle (alle DTEs 0,
V = 0), Befehlspuffer, Ereignispuffer, `early_enable_iommu` (#8-12), ohne
einen einzigen uebersetzten DTE. Hinter `set gpu_iommu on`, ausgeloest beim
ersten `protect`. *Test:* `gpu iommu` zeigt „AMD-Vi an, 0 Domaenen";
ein `COMPLETION_WAIT` kehrt zurueck (Semaphorwert erreicht), ein
`INVALIDATE_ALL` laeuft ohne `ILLEGAL_COMMAND`; Tastatur, Touchpad, NVMe,
USB-Netz, Ton laufen wie vorher (der Lauf ist der Beweis, dass V = 0 wirklich
Durchgang ist). Ereignisprotokoll leer.

**Stufe 2 — die GPU eingesperrt.** v1-Tabelle, `set_dte_entry` fuer
04:00.0, `grant`/`revoke` mit Flush und Completion-Wait (#14-17), IVMD der
devid als Grundfreigabe. `protected()` antwortet wahr, also wird
`npk_pci_enable_bus_master` von `gpu_amd_renoir` zum Einsperren. *Test:*
`gpu_amd_renoir` Stufe 0/1 laeuft unveraendert weiter (es macht noch kein
DMA); ein Ereignis `IO_PAGE_FAULT` erscheint nur, wenn das Modul etwas nicht
Freigegebenes beruehrt — **jedes** Ereignis hier ist ein Befund ueber die
Firmware oder das Modul, nicht Laerm.

**Stufe 3 — `gpu iommu test` auf AMD.** `iommu_probe` braucht eine Szene,
also eine Engine, die liest und schreibt. Bis `gpu_amd_renoir` SDMA hat
(GPU_AMD_LINUX_MAP §1 Stufe 4), ein kleinerer Beweis mit derselben Form:
das Modul laesst die Karte eine freigegebene Seite schreiben (z. B. ein
Fence-/Writeback-Ziel der Ringe, das amdgpu ohnehin anlegt), dann
`revoke`, dasselbe nochmal. *Erwartet:* erster Lauf Wert kommt an, 0
Ereignisse; zweiter Lauf Wert kommt nicht an, `IO_PAGE_FAULT` mit devid
0x0400, Schreib-Flag, Adresse **in** der widerrufenen Seite; danach `grant`
und wieder gruen. Sobald SDMA laeuft, ist es woertlich `gpu iommu test`.

**Stufe 4 — `native_device()` am IdeaPad.** `gpudev::native_device` fragt
die Weiche statt `vtd`; nichts sonst aendert sich. *Test:* `gpu` sagt
„native contexts for guests" statt „need the IOMMU".

**Stufe 5 — Interrupt-Remapping** (beide Hersteller): IRTE je Geraet,
`IV = 1`, IOAPIC-/HPET-Eintraege aus `IVHD_DEV_SPECIAL`. Eigenes Papier;
erst danach ist eine eingesperrte Karte auch fuer Interrupts eingesperrt.

**Stufe 6 — weitere Geraete, Linux-Vorgabe.** Jedes Geraet bekommt beim
Bus-Mastering eine Domaene oder Identitaet, unbekannte devids blockiert
(`init_device_table_dma`). Nur mit Stufe 1-4 als Vorlauf, und Geraet fuer
Geraet (xHCI zuerst — sein DMA ist am besten sichtbar).

---

## 5 — Risiken

| Risiko | Was bricht | Wie Linux es vermeidet / unser Schutz |
|---|---|---|
| Einschalten mit DTEs, die nicht 0 sind | V = 1 ohne Tabelle blockiert Geraete, die laufen (NVMe = Plattenausfall, xHCI = keine Tastatur) | Linux nutzt nur sauber genullte, eigene Tabellen; wir schreiben nur IVHD-Flags (ohne V) und den einen GPU-DTE. `init_device_table_dma` NICHT vor Stufe 6. |
| Firmware hat die IOMMU schon an | Unsere Register ueberschreiben eine laufende Uebersetzung | `init_translation_status` + `iommu_disable` (1981); Stufe 0 meldet `IOMMU_EN`. |
| IVHD-Typen doppelt | zwei Instanzen derselben IOMMU, doppelter Befehlspuffer | `get_highest_supported_ivhd_type` (2011). |
| Alias faltet GPU mit einem Nachbarn | xHCI/Audio auf Bus 04 erben den GPU-DTE und verlieren ihren DMA | `clone_aliases`/`alias_table` — Stufe 0 druckt Aliase; gibt es einen fuer 04:00.0, sperrt `protect` mit Meldung ab (wie VT-d bei geteilter Einheit). |
| Scanout | Bild friert/schwarz, wenn die Anzeige aus nicht freigegebenem Speicher liest | Erwartung: Carve-out geht nicht durch die IOMMU (§3, zu messen). Liest DCN aus Systemspeicher, muss der Scanout-Puffer freigegeben sein — IVMD + `grant` aus dem Modul. GOP-Framebuffer liegt im Carve-out (BAR 0). |
| DTE reisst beim Schreiben | IOMMU liest halben DTE → `ILLEGAL_DEV_TABLE_ENTRY` oder falsche Wurzel | `update_dte256`: zwei `cmpxchg16b`, V zuletzt/zuerst, dann `INVALIDATE_DEVTAB_ENTRY` + Completion-Wait. CMPXCHG16B ist Pflicht (3251). |
| `revoke` kehrt zu frueh zurueck | Seite geht an den Allokator, die Karte schreibt noch hinein | `INVALIDATE_IOMMU_PAGES` mit PDE-Bit, dann `COMPLETION_WAIT` und auf den Semaphor warten (1404). Erst dann `deallocate_contiguous`. |
| ATS an | Karte cached Uebersetzungen bzw. schickt vorab uebersetzte Adressen an der Tabelle vorbei | `DTE.IOTLB = 0`, ATS in der PCIe-Capability nie einschalten (§2.7). |
| Befehls-/Ereignispuffer laeuft voll | Befehle haengen, Ereignisse gehen verloren (`EVT_OVERFLOW`, types.h:128) | Platzpruefung mit Timeout (1338); Ereignisse bei jedem `poll` leeren; Ueberlauf in `gpu iommu` zaehlen, `amd_iommu_restart_event_logging` (init.c:783) portieren. |
| MSI ohne IR | eingesperrte Karte loest fremde Vektoren aus | benannt, Stufe 5. |
| Tabelle zu flach | `grant` oberhalb 512 GB scheitert | `increase_address_space` (io_pgtable.c:106) bis `amd_iommu_hpt_level`; HT-Loch 0xFD_0000_0000-0xFF_FFFF_FFFF (`HT_RANGE_*`) liegt nie in unserem RAM. |
| Rueckweg | nach einem Fehler weiterarbeiten mit halb eingeschalteter IOMMU | wie Linux `state_next`-Fehlerpfad (3546-3561): DTEs nullen, Caches flushen — also Durchgang, nicht Sperre. Schalter `gpu_iommu` bleibt Vorgabe aus, OTA-Rueckzug wie immer. |
| Die IOMMU-Funktion 00:00.2 | ein Modul beansprucht sie und schreibt in ihre Register | in `pci` als Kernelbesitz markieren; ihr MMIO steht schon in `platform_mmio_ranges`. |

**Sicherheits-Checkpoint je Stufe:** „Kann ein WASM-Modul durch diese
Aenderung aus der Sandbox?" — Stufe 1 aendert fuer kein Geraet die
Erreichbarkeit (V = 0 ueberall); ab Stufe 2 wird die Karte strenger, nie
lockerer; Befehls-, Ereignis- und Geraetetabelle sowie die Seitentabellen
liegen in Kernelspeicher, der nie freigegeben wird und den nie ein Modul
erreicht (insbesondere nicht als `grant` — das muss `grant` pruefen, wie es
bei VT-d fuer Wurzel- und Kontexttabelle gilt).

---

## 6 — Vertrauensmodell: was die IOMMU auf einer APU schuetzt und was nicht

Die IOMMU sperrt die **DMA der Karte** ein: was die GPU-Engines (GFX, SDMA,
VCN, IH, DCN aus Systemspeicher) lesen und schreiben, geht durch den DTE von
04:00.0. Das ist die Gefahr, die zaehlt, sobald Gaeste ueber native Kontexte
Befehlspuffer einreichen: ein Befehlsstrom aus dem Gast darf die Karte nur
auf freigegebene Seiten lenken.

**Nicht** eingesperrt ist das Modul `gpu_amd_renoir` selbst, und das laesst
sich auf einer APU auch nicht mit der IOMMU leisten:

- BAR5 enthaelt das PCIe-Index/Data-Paar (`PCIE_INDEX2`/`PCIE_DATA2`), und
  das erreicht den SMN des ganzen Chips — Stufe 2 schreibt darueber ein
  NBIO-Strap auf `0x10134c10`.
- Die SMU (MP1) bekommt vom Treiber DRAM-Adressen genannt
  (`smu_v12_0_set_driver_table_location`, `SetDriverDramAddrHigh/Low`) und
  greift selbst darauf zu; die PSP (MP0) ebenso fuer ihren Ring und die TMR.
  Beide sind Koprozessoren des SoC und stehen nicht hinter dem DTE der GPU.

Ein Filter auf den Index-Registern wuerde also nichts schliessen: ein
boeswilliger Treiber naehme den Weg ueber die SMU. **Der AMD-Treiber ist
vertrauter Code** — von uns portiert, mit dem Update-Schluessel signiert —,
so wie amdgpu im Linux-Kernel. Die WASM-Sandbox haelt Fehler des Moduls aus
dem Kernelspeicher (kein Zeiger, keine Seitentabelle), die IOMMU haelt die
Karte aus fremdem Speicher. Ein Modul, das diese Karte bekommt, muss
signiert sein; ein fremdes Modul fuer 1002:164c waere eine Vertrauensfrage,
keine Sandboxfrage. Beim NUC gilt dasselbe in kleinerem Umfang (GuC/HuC,
GGTT).
