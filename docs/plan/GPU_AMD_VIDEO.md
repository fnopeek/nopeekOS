# gpu_amd_renoir Stufe 6 — Video: VCN 2.2 und JPEG 2.0

**Stand:** 2026-10-22 · Modul `gpu_amd_renoir` 0.17.1 · Kernel 0.664.0
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
