# gpu_amd_renoir Stufe 6 — Video: VCN 2.2 und JPEG 2.0

**Stand:** 2026-10-22 · Modul `gpu_amd_renoir` 0.21.0 (V1–V3 ✅ Geraet soweit ohne Gast messbar, V4 gebaut) · Kernel 0.666.0 · Bundle firefox-0.1.3
**Vorbild:** Linux 6.18.55 `vcn_v2_0.c`, `jpeg_v2_0.c`, `amdgpu_vcn.c`,
`amdgpu_jpeg.c`; Gast: Mesa 26.2.4 `radeonsi` VA-API (`gallium-va`).
**Daneben:** `GPU_AMD_NATIVE.md` (der Weg der Gast-ccmds), `GPU_AMD_LINUX_MAP.md`
§1 Zeile 6.

## 0. Was es heute gibt

Im Modul: `vcn_v2_0_early_init`, `vcn_v2_0_sw_init` (VCPU-Puffer, DPG-SRAM,
Firmware ueber den PSP, IRQ-Quellen, Ringe), `jpeg_v2_0_early_init`/`sw_init`.
Die SMU schaltet VCN/JPEG in `smu_hw_init` an (`renoir_dpm_set_vcn_enable`).
`hw_init` von VCN und JPEG fehlt; `amdgpu_kms` meldet deshalb fuer
`VCN_DEC`/`VCN_ENC`/`VCN_JPEG` keine verfuegbaren Ringe, und Firefox im Gast
sagt `HARDWARE_VIDEO_DECODING = unavailable`.

Der Weg vom Gast zum Ring ist schon generisch: Mesa bildet jeden HW-IP-Typ
auf einen virtio-Ring ab (`virtio_ring_mapping[ip_type] + ring`),
virglrenderer und der Kernel ebenso (Zeitleiste je Ring). Es fehlt die
Hardware hinter dem Ring und der VA-API-Treiber im Gast.

## 1. Was Renoir verlangt

`soc15_common_early_init` fuer GC 9.3.0: `cg_flags` mit `VCN_MGCG`,
`JPEG_MGCG`; `pg_flags` mit `VCN`, `JPEG`, **`VCN_DPG`**. Also:

- **VCN startet im DPG-Modus** (`vcn_v2_0_start_dpg_mode`,
  `vcn_v2_0_mc_resume_dpg_mode`, `vcn_v2_0_clock_gating_dpg_mode`, indirektes
  SRAM ueber den PSP — `psp_update_vcn_sram`), nicht ueber `vcn_v2_0_start`.
- **Strom nach Benutzung**: `hw_init` testet nur die Ringe; jede Benutzung
  geht ueber `amdgpu_vcn_ring_begin_use` (Idle-Arbeit abbrechen,
  `set_powergating_state(UNGATE)` → Start) und `amdgpu_vcn_ring_end_use`
  (`idle_work` nach 1 s: Zaeune zaehlen, DPG pausieren/entpausieren,
  `GATE` → `vcn_v2_0_stop_dpg_mode`, SMU `dpm_enable_vcn(false)`). JPEG
  genauso (`amdgpu_jpeg_ring_begin_use`/`end_use`, `idle_work`).
- **Dekoder und Encoder laufen im VM-Modus** (`*_ring_vm_funcs`): die VCPU
  liest ueber die VMID des Auftrags, Linux prueft keine Befehle. **JPEG hat
  eine Pruefung**: `amdgpu_jpeg_dec_parse_cs` (nur Registerschreiben in
  einem erlaubten Bereich). Die gehoert mit.

## 2. Schritte, jeder mit einem Ergebnis

| Schritt | Inhalt | Pruefung | Ergebnis am Geraet |
|---|---|---|---|
| **V1** VCN | `vcn_v2_0_hw_init`, `start_dpg_mode`, `stop_dpg_mode`, `pause_dpg_mode`, `mc_resume_dpg_mode`, `clock_gating_dpg_mode`, `set_powergating_state`, `set_clockgating_state`, Ring-Ops (dec/enc: rptr/wptr, `emit_fence`, `emit_ib`, `emit_vm_flush`, `emit_reg_wait`, `insert_*`), `process_interrupt`, `amdgpu_vcn_dec_ring_test_ring`, `amdgpu_vcn_enc_ring_test_ring`, IB-Tests (`amdgpu_vcn_dec_ring_test_ib`, `_enc_`), `begin_use`/`end_use`/`idle_work_handler`, `hw_fini`/`suspend` | Orakel `main_vcn.c` + `vcn4_check.rs`: jede Registerschreibung gegen Linux, wie `sdma4` | `ring tests:` und `ib tests:` mit `vcn_dec`, `vcn_enc0/1` |
| **V2** JPEG | `jpeg_v2_0_hw_init`, `start`/`stop`, PG/CG, Ring-Ops, Ringtest, IB-Test, `begin_use`/`end_use`, `process_interrupt`, `amdgpu_jpeg_dec_parse_cs` | Orakel wie V1 | `jpeg_dec` in den Tests |
| **V3** Gast-ccmds | `HW_IP_INFO`/`HW_IP_COUNT` fuer VCN/JPEG (verfuegbare Ringe), `VIDEO_CAPS` (`rn_video_codecs_*`), CS-Einreichung auf VCN/JPEG-Entitaeten (`amdgpu_ctx_get_entity`), `parse_cs` im CS-Weg, Haenger: Ringe in `amdgpu_job_timedout` (VCN 2.0 hat keinen Queue-Reset → voller Recover) | `stage1_run` mit einem Gast-Dekodierauftrag auf der Ersatz-GPU | `vainfo` im Gast nennt radeonsi und die Profile |
| **V4** Gast | Mesa-Bundle mit `-Dgallium-va=enabled` (radeonsi `_drv_video.so`), init waehlt `LIBVA_DRIVER_NAME` nach Hersteller (heute fest `iHD`) | `vainfo` im Image-Bau | Firefox `HARDWARE_VIDEO_DECODING = available`, ein 1080p-Video mit `vcn` im Lastbild |

## 3. Sicherheit

- Dekoder/Encoder lesen und schreiben nur ueber die VM des Auftrags (VMID
  aus `amdgpu_vm_flush`), wie GFX — die Grenze aus 5b gilt.
- JPEG schreibt Register aus dem Befehlsstrom: ohne `parse_cs` koennte ein
  Gast beliebige Register der Karte setzen. **Ohne V2s Pruefung keine
  JPEG-Ringe fuer den Gast.**
- TMZ (`secure_submission_supported`) bleibt aus (`tmz_enabled` false).

## 4. Offen vor V1

- Das DPG-SRAM laedt der PSP: `vcn_v2_0_start_dpg_mode` ruft
  `amdgpu_vcn_psp_update_sram` → `psp_execute_ip_fw_load` mit einem
  Augenblicks-`amdgpu_firmware_info` (`AMDGPU_UCODE_ID_VCN0_RAM`, Groesse =
  geschriebene SRAM-Bytes). Der Port von `psp_execute_ip_fw_load` nimmt heute
  einen Index in `firmware.ucode`; er muss die Struktur selbst nehmen, und
  `psp_get_fw_type` muss `VCN0_RAM` kennen.
- Die Idle-Arbeit braucht die verzoegerte Arbeit aus `workqueue.rs` mit
  1-s-Frist; der Szenen-/ccmd-Takt des Moduls muss sie ausfuehren.

## 5. V1 gebaut (Modul 0.18.0)

`vcn_v2_0.rs`: `hw_init`/`hw_fini`/`suspend`/`resume`, der DPG-Start mit
indirektem SRAM (`start_dpg_mode`, `mc_resume_dpg_mode`,
`clock_gating_dpg_mode`, `enable_static_power_gating`, SRAM ueber
`amdgpu_vcn_psp_update_sram` → `psp_execute_ip_fw_load`, das dafuer jetzt
wie Linux eine `amdgpu_firmware_info` nimmt), `stop_dpg_mode`,
`pause_dpg_mode` (programmiert die Encoder-Ringe), `set_pg_state`, die
Ring-Ops von Dekoder und Encoder, `process_interrupt`, Ringtests, IB-Tests
(Dekoder: Create-/Destroy-Nachricht, Encoder: Session auf/zu),
`begin_use`/`end_use`/`idle_work_handler` mit dem Video-Profil der SMU
(`amdgpu_dpm_switch_power_profile`). Der Start ohne DPG ist benannt und
nicht gebaut: Renoirs `pg_flags` tragen `VCN_DPG` fest.

In der Ring-Schicht neu, beides auch Linux-treu fuer die alten Ringe:
`begin_use` NACH `wptr_old` in `amdgpu_ring_alloc`, `end_use` in
`amdgpu_ring_commit`, `insert_start`/`insert_end` in `amdgpu_ib_schedule`,
und `emit_pipeline_sync` getrennt von `emit_vm_flush` (VCN hat nur das
zweite; `amdgpu_vm_flush` laeuft jetzt fuer jeden Job wie in Linux).

**Geprueft:** `oracle/main_vcn.c` faehrt Linux' eigene Funktionen
(wortwoertlich extrahiert) durch hw_init, Leerlauf, Benutzung, hw_fini;
`hostcheck/vcn4_check.rs` den Port. Jede Registerschreibung, jedes Doorbell
mit seinen Ring-Dwords, Pause-/Stromzustand, die Queue-Modi der Firmware
und die 264 Byte DPG-SRAM sind in allen drei Saaten gleich
(`vcn (ones|zeros|hash): port = Linux (46 register writes, 3 doorbells)`).
Die Ersatz-GPU hat dafuer ein VCN-Modell (Strom-FSM, Pause-ACK, Dekoder-
und Encoder-Ringe mit Zaun und Trap, 40-Bit-Adressen ueber den Hub) und
32-Bit-Doorbells; `stage1_run` faehrt die IB-Tests: `vcn_dec+ vcn_enc0+
vcn_enc1+`.

**Am Geraet zu lesen:** `ring tests:` und `ib tests:` mit `vcn_dec+`,
`vcn_enc0+`, `vcn_enc1+`. Scheitert ein VCN-**Ringtest**, scheitert wie in
Linux die ganze Initialisierung des Moduls (dann `install` der 0.17.1);
ein gescheiterter IB-Test nimmt nur den Ring aus dem Dienst. Den Gaesten
meldet `HW_IP_INFO` die VCN-Ringe noch nicht (V3).

**Am Geraet (IdeaPad, 2026-10-22): ✅** `ring tests:` und `ib tests:` beide mit
`vcn_dec+ vcn_enc0+ vcn_enc1+`; danach Szenen und native Kontexte wie
vorher.

**Offen in V1:** `amdgpu_vcn_ring_reset` (Queue-Reset bei Haenger) — heute
faellt ein haengender VCN-Auftrag auf den vollen Recover; gehoert zu V3.

## 6. V2 gebaut (Modul 0.19.0)

`jpeg_v2_0.rs`: `hw_init`/`hw_fini`/`suspend`/`resume`, `start`/`stop`
(Strom ueber die PGFSM, Takt-Gating, JRBC-Ring), `set_powergating_state`,
die Ring-Ops (PACKETJ: `insert_start`/`insert_end`, `emit_fence`,
`emit_ib` mit IB-VMID 0, `emit_reg_wait`, `emit_vm_flush`, `emit_wreg`,
NOP), `process_interrupt`, Ring- und IB-Test (das Pitch-Register),
`begin_use`/`end_use`/`idle_work` und **`amdgpu_jpeg_dec_parse_cs`**: nur
Registerschreiben (TYPE0, Bedingung 0) und -warten (TYPE3, Bedingung 3)
in `0x4000..=0x41c2` ohne den Atomic-Bereich `0x4120..=0x412a`, dazu der
eine NOP; alles andere `-EINVAL`.

**Warum die Pruefung Pflicht ist:** mit `parse_cs` gibt `emit_ib` die
IB-VMID 0 — die Engine liest den IB ueber den Kernel-Adressraum. Linux
kopiert dafuer in `amdgpu_cs_patch_ibs` den IB des Benutzers in einen
Kernel-IB und prueft die Kopie. Ohne genau diesen Weg (V3) darf kein
Gast-IB auf den JPEG-Ring.

**Geprueft:** `oracle/main_jpeg.c` + `hostcheck/jpeg4_check.rs` wie bei VCN
(`jpeg (ones|zeros|hash): port = Linux`), und beide beurteilen dieselben
588 Befehlsstroeme — die Grenzen des Bereichs und der Atomic-Luecke, jede
Paketart mit richtiger und falscher Bedingung, ein gesetztes
Reserviert-Feld, ungerade Laengen, dazu 400 gezogene: 561 abgewiesen, 27
angenommen, in allen Saaten gleich. Die Ersatz-GPU hat ein JPEG-Modell;
und ihr Strom-Modell wurde dabei richtig gestellt (ein Feld auf 1 heisst
„einschalten“ und liest dann 0).

**Am Geraet (IdeaPad, 2026-10-22): ✅** `ring tests:`/`ib tests:` beide mit
`jpeg_dec+`. Auch hier: ein gescheiterter Ringtest beendet wie in Linux
die Initialisierung (dann `install` 0.18.0).

## 7. 0.19.1: die Luecke in 0.19.0

**0.19.0 liess sie offen.** Der JPEG-Ring war bereit, und der CS-Weg
nahm einen Gast-Auftrag mit `ip_type` `AMDGPU_HW_IP_VCN_JPEG` an, ohne
`parse_cs` zu kennen. Der Gast-IB lief dann ungeprueft mit VMID 0, also
ueber den Kernel-Adressraum der GPU, und konnte jedes Register schreiben.

**Jetzt** (`amdgpu_cs_pass2`): ein Ring mit `parse_cs` weist jeden
Auftrag mit `-EINVAL` ab, bis der Kopier- und Pruefweg aus V3 steht. Dazu
die Regel aus Linux, dass die MM-Engines keinen Benutzer-Fence koennen
(`no_user_fence` fuer VCN und JPEG). `stage1_run` schickt einen
Gast-Auftrag auf `jpeg_dec`, und `check.sh` faellt durch, wenn er nicht
abgewiesen wird (`guest IB on jpeg_dec: refused`).

## 8. V3, erster Teil (Modul 0.20.0)

**Gebaut:**

- `HW_IP_INFO` zaehlt die bereiten Ringe fuer `VCN_DEC`, `VCN_ENC` und
  `VCN_JPEG` wie Linux (Harvest-Maske, `sched.ready`). Damit meldet der
  Gast 1 + 2 + 1 Ringe, und `client_open` legt dafuer Zeitlinien an.
- `amdgpu_ctx_get_hw_prio` gibt `VCN_ENC` (und `VCE`) die Ringprioritaet
  aus `amdgpu_ctx_sched_prio_to_ring_prio`; `amdgpu_ctx_init_entity`
  waehlt fuer VCN/UVD mit `drm_sched_pick_best` EINEN Scheduler, weil die
  Engine Kontext zwischen abhaengigen Auftraegen haelt.
- CS-Pruefungen in Linux' Reihenfolge, mit der Obergrenze je Ringart
  (`amdgpu_cs_ib_packet_size_max_dw`: MM-Engines 0x7ffff0 Dwords).
- Haenger: **die Annahme in §2 war falsch**, VCN 2.0 HAT einen Queue-Reset.
  `amdgpu_vcn_ring_reset` → `amdgpu_vcn_reset_engine` (alle Ringe der
  Instanz anhalten, `vcn_v2_0_reset` = stop + start, Ringtests, Fences
  abschliessen, Scheduler wieder an) und `jpeg_v2_0_ring_reset` (mit dem
  Reset-Helfer, wie die Compute-Ringe).
- `VIDEO_CAPS` war schon da.

**Geprueft:** `stage1_run` schickt einen Gast-Auftrag auf `vcn_enc0`, den
der Encoder der Ersatz-GPU nie abarbeitet. Die Ersatz-GPU laeuft dafuer
jetzt die Pakete eines Encoder-IBs (jedes beginnt mit seiner Groesse in
Bytes, wie Linux' eigener IB-Test sie baut) ueber die VM des Auftrags auf
dem MM-Hub ab; ein Paket der Groesse 0 kommt nie weiter. Der Auftrag
laeuft in den Timeout, der Ring wird zurueckgesetzt, der schuldige
Auftrag meldet -62, und der IB eines fuenften Kontexts laeuft danach auf
dem Encoder. `check.sh` faellt durch, wenn das nicht so kommt (`guest hang
on vcn_enc0: ring reset, the next IB runs`). Alle 40 Orakelvergleiche
bleiben gleich mit Linux.

**Offen in V3:** der Kopier- und Pruefweg fuer JPEG (`amdgpu_cs_patch_ibs`:
Kernel-IB der Groesse `ib_bytes` aus dem verzoegerten Pool,
`amdgpu_cs_find_mapping`, den Gast-IB kopieren, `parse_cs` auf der Kopie).
Dafuer muss das Modul Gast-Objekte lesen koennen: im VRAM ueber das
Aperture-Fenster, Objekte im Systemspeicher brauchen eine Leseschnittstelle
im Kernel. Bis dahin meldet `HW_IP_INFO` den JPEG-Ring wie Linux, und
jeder Auftrag darauf wird abgewiesen.

**Am Geraet zu lesen:** unveraendert `ring tests:`/`ib tests:` mit allen
vier Video-Ringen. Sichtbar wird V3 erst mit V4 (`vainfo` im Gast).

## 9. V3, zweiter Teil: JPEG ueber eine Kernel-Kopie (Kernel 0.665.0, Modul 0.21.0)

**Gebaut**, nach `amdgpu_cs.c`: `amdgpu_cs_p1_ib` holt fuer einen Ring
mit `parse_cs` einen Kernel-IB der Groesse `ib_bytes` aus dem
verzoegerten Pool; `amdgpu_cs_patch_jobs` → `amdgpu_cs_patch_ibs` sucht
mit `amdgpu_cs_find_mapping` (`amdgpu_vm_bo_lookup_mapping`, das Objekt
muss eines dieses CS sein, VRAM wird zusammenhaengend, GART-Adresse)
die Abbildung, prueft die Laenge gegen ihr Ende, kopiert den Gast-IB in
den Kernel-IB, laesst `amdgpu_jpeg_dec_parse_cs` die KOPIE pruefen und
setzt die Adresse des IBs auf die Kopie. Die Engine liest also nie, was
der Gast danach noch aendern koennte. Die Sperre aus 0.19.1 ist weg.

**Neu im Kernel: `npk_gpudev_tt_read(tt, offset, out, len)`.** Gast-
Objekte im Systemspeicher hatten im Modul keine Kernel-Abbildung
(`amdgpu_bo_kmap` gab `-ENOMEM`). Die Funktion liest nur GTT-Seiten, die
das Modul selbst fuer einen Client angelegt hat (`tt_alloc`) und noch
haelt, mit Grenzpruefung und unter der TT-Sperre, damit die Seiten
waehrend des Kopierens nicht frei werden. Im Modul ist das die neue
Abbildungsart `amdgpu_kptr::client_tt`, nur lesend.

**Wie Linux, auch wo es auffaellt:** der Versatz der Abbildung
(`m->offset`) geht in Linux nicht in die Leseadresse ein; hier ebenso.

**Geprueft:** `stage1_run` schickt den PM4-IB auf den JPEG-Ring (die
Pruefung lehnt ihn ab, `Unknown packet type 12`) und einen IB aus einer
Pitch-Schreibung und einem NOP, der aus der Kopie laeuft (die Ersatz-GPU
liest ihn an einer Kernel-Adresse, die Fence meldet Erfolg). `check.sh`
faellt durch, wenn eins davon nicht so kommt. Alle 40 Orakelvergleiche
gleich.

**Reihenfolge beim Update:** erst `update` (Kernel 0.665.0), dann
`install gpu_amd_renoir` — Modul 0.21.0 importiert die neue Funktion.

**Damit ist V3 bis auf den Geraetetest gebaut.** Naechst V4: Mesa im
Gast mit `gallium-va`, `LIBVA_DRIVER_NAME` nach Hersteller.

**Am Geraet (IdeaPad, 2026-10-22): ✅** 0.21.0 auf Kernel 0.665.0 laedt
(der neue Import ist da), Ring- und IB-Tests unveraendert gruen.

## 10. V4: der Gast (Kernel 0.666.0, microvm-init 0.15.0, Bundle firefox-0.1.3)

- **Mesa im Gast mit `-Dgallium-va=enabled`.** `radeonsi_drv_video.so` ist
  wie bei Alpine ein Link auf `libgallium`, und geht in das Mesa-Paket
  (`microvm/linux/mesa/build.sh`). Dabei Mesa 26.2.4 → 26.2.5, weil
  Alpine edge nachgezogen hat (Pruefsumme aus Alpines APKBUILD).
- **Der Wirt nennt dem Gast die Karte:** `npkgpu=amdgpu` oder
  `npkgpu=i915` auf der Kommandozeile, aus `gpu_backend::native_protocol`
  — derselben Wahl, mit der `gpu_backend::reset` den Backend anlegt.
  (0.666.0 las den Capset; der Backend entsteht aber erst NACH der
  Kommandozeile, und der erste Geraetelauf hatte kein `npkgpu=`. 0.666.1.) Das Render-Device im Gast
  heisst `virtio_gpu` und nennt keinen VA-Treiber.
- **microvm-init** setzt fuer Firefox `LIBVA_DRIVER_NAME=radeonsi` bei
  `npkgpu=amdgpu`, sonst wie bisher `iHD`. Neu `npkvainfo` auf der
  Kommandozeile: `vainfo` mit diesem Treiber als `[va]`-Zeilen.
- Das Image nimmt den aktuellen Stand von Alpine edge (Firefox 154.0
  unveraendert; Mesa, musl, nss u.a. neuer, `packages-firefox.txt`).

**Am Geraet zu lesen** (`set microvm_cmdline npkvainfo npksupport`, dann
`firefox`): `[va]` nennt `Mesa Gallium driver … for AMD Radeon` und eine
Liste von `VAProfile…` (H.264, HEVC, VP9, JPEG); `[about]` meldet
`HARDWARE_VIDEO_DECODING` als verfuegbar. Dann ein Video abspielen; das
Lastbild zeigt Arbeit auf `vcn_dec` statt CPU-Last.

**Gegen Mesa 26.2.5 gelesen** (`src/amd/common/virtio`,
`winsys/amdgpu`, `ac_gpu_info.c`, `frontends/va`):

- Ring-Nummern: Mesa zaehlt `AMD_NUM_IP_TYPES` = 10 IP-Typen der Reihe
  nach mit ihren `available_rings` ab Ring 1 (`virtio_ring_mapping`),
  genau wie `client_open` seine Zeitlinien (`AMDGPU_HW_IP_NUM` = 10).
  Ein VCN-Auftrag landet also auf der Zeitlinie, deren Folgenummer der
  Gast erwartet.
- Benutzer-Fence: Mesa haengt `AMDGPU_CHUNK_ID_FENCE` nur an GFX,
  Compute und SDMA (`amdgpu_cs_has_user_fence`) — die Ablehnung auf den
  MM-Engines trifft keinen echten Auftrag.
- Jede `AMDGPU_INFO_*`-Frage, die `ac_gpu_info`, die Winsys und der
  VA-Frontend stellen (darunter `FW_VCN`, `VIDEO_CAPS`, `HW_IP_INFO` mit
  `ip_discovery_version` 2.2), beantwortet `amdgpu_kms`.

**Ungeprueft bleibt** der Dekoder selbst: ob die Firmware die Nachrichten
von radeonsi ueber die VM des Gasts liest und schreibt wie erwartet. Das
zeigt erst der Geraetelauf.

## 11. Der erste Geraetelauf mit einem Video (Kernel 0.666.1, Modul 0.21.0)

Firefox meldet `H264 HWDEC HWENC`, `VP9 HWDEC`, `HEVC HWDEC HWENC`; VP8 und
AV1 nur in Software — das ist Renoirs VCN 2.2. Der erste Dekodierauftrag
eines Gasts aber haengt:

    ring vcn_dec timeout, signaled seq=2, emitted seq=4
    Starting vcn_dec ring reset
    Register(0) [mmUVD_POWER_STATUS] failed to reach value 0x00000001 != 0x00000002
    Register(0) [mmUVD_RBC_RB_RPTR] failed to reach value 0x00000060 != 0x00000000
    Ring vcn_dec reset succeeded

`seq=2` sind die zwei Fences des IB-Tests beim Laden: **kein Auftrag eines
Gasts ist auf `vcn_dec` je fertig geworden**, auch nicht im Lauf davor (dort
spielte das Video in Software). Der Block ist an (`POWER_STATUS` 2 =
`UVDM_UVDU_PWR_ON`), die RBC hat vom Ring nichts gelesen (`RPTR` 0 bei
`WPTR` 0x60). Danach lehnt `amdgpu_ctx_get_entity` jeden weiteren Auftrag
des Kontexts mit ETIME ab (`drm_sched_entity_error`) — wie Linux; Mesas
Dekoder legt keinen neuen Kontext an, also bleibt das Video stehen.

Neu gegenueber den Ringtests ist an einem Gastauftrag der VM-Flush auf dem
MMHUB (Semaphor, Invalidierung, `REG_READ_COND_WAIT`) und die VMID des
Gasts; beides lief auf dem Geraet nie.

**Modul 0.22.0** portiert dafuer den Core-Dump von `amdgpu_job_timedout`
(`amdgpu_job_do_core_dump`): `dump_ip_state` von VCN und JPEG
(`vcn_reg_list_2_0`, `jpeg_reg_list_2_0`), dann aus `amdgpu_devcoredump_read`
der Ring, der letzte VM-Fehler, die IP-Register und der Ringinhalt. Ohne
devcoredump-Datei geht das ins Log, und vom Ring nur der Teil von 32
Dwords vor `rptr` bis `wptr` (hoechstens 256). Die Dumps von GFX und SDMA
sind nicht portiert.

**Der Dump (Modul 0.22.0, 2026-10-22)** sagt, dass die RBC nach dem Start
**keinen einzigen Befehl** ausgefuehrt hat: `GPCOM_VCPU_DATA0/DATA1/CMD`
stehen auf 0, obwohl der erste Befehl im Ring sie beschreibt; `RBC_RB_RPTR`
0, Hardware-`WPTR` 0x60 (die Tuerklingel kam an), `UVD_STATUS` 0x26 ohne
`RBC_BUSY`. Der Ring ist richtig gebaut (VM-Flush mit Semaphor, Fence 3,
`PACKET_START`, IB auf VMID 1 an `0xffff8001_00220000`, Fence 4,
`PACKET_END`) — er wird nur nie gelesen. Damit haengt es nicht am
VM-Flush, sondern am Start des Blocks.

Der Start nach einem Ruhen ist der Weg, den das Modul bis dahin nie fuhr:
die IB-Tests liefen gleich nach `late_init`, solange VCN noch vom
Ringtest an war. Linux faehrt sie `AMDGPU_RESUME_MS` (2 s) spaeter aus
`delayed_init_work` — nach dem Idle-Work, das VCN und JPEG bei 1 s
gated. **Modul 0.23.0** tut das ebenso: der IB-Test von `vcn_dec` startet
den Block dann aus dem Ruhezustand, genau wie der erste Auftrag eines
Gasts. Geht er schief, zeigt das der Bootlog ohne Firefox.

**Lauf mit 0.24.0:** derselbe Haenger, und `signaled seq=2` mit 0.23.0+
heisst jetzt: die IB-Tests von `vcn_dec` liefen 2 s nach dem Laden, also
NACH dem Idle-Gate, und gingen durch. Der Start aus dem Ruhezustand ist es
nicht. Was ein Gastauftrag darueber hinaus verlangt, ist der VM-Flush auf
dem MMHUB (Semaphor, Invalidierung, PTB schreiben und zuruecklesen, PASID-
Tabelle des IH) und die VMID des Gasts. **Modul 0.25.0** fragt das Stueck
fuer Stueck ab, auf VMID 0 mit der Tabelle, die schon da ist: jede Probe
ist der Ringtest (`PACKET_START` … `SCRATCH9`) mit einem Teil des Flushs
dazwischen, und `vcn_dec probe <teil>: passed|STUCK` steht im Ladelog.
Die Probe ist kein Linux und faellt wieder weg, sobald der Teil gefunden
ist.

**Lauf mit 0.25.0:** alle Proben auf VMID 0 gehen durch (Semaphor,
Invalidierung mit Ack, Flush, PASID-Tabelle). `RBC_RB_RPTR` blieb dabei
auf 0x40 stehen, waehrend `SCRATCH9` ankam — das Register sagt im
DPG-Betrieb also nicht, wie weit die Firmware ist; das `RPTR 0` der Dumps
beweist nichts. Der Gastauftrag haengt vor seiner ersten Fence, also im
Flush auf **VMID 1**. **Modul 0.26.0** probt dasselbe auf VMID 1 (PTB
schreiben und zuruecklesen, Invalidierung mit Ack-Bit 1, der ganze
`emit_vm_flush` des Rings, PASID-Tabelle) und den Flush in der Reihenfolge
von `amdgpu_ib_schedule`, also VOR `PACKET_START`.
