# gpu_amd_renoir Stufe 6 — Video: VCN 2.2 und JPEG 2.0

**Stand:** 2026-10-22 · Modul `gpu_amd_renoir` 0.19.0 (V1 ✅ Geraet, V2 gebaut) · Kernel 0.664.0
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

**Am Geraet zu lesen:** `ring tests:`/`ib tests:` zusaetzlich mit
`jpeg_dec+`. Auch hier: ein gescheiterter Ringtest beendet wie in Linux
die Initialisierung (dann `install` 0.18.0).

