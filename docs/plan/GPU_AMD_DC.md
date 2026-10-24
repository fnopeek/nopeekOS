# GPU_AMD_DC — Display Controller (DCN 2.1 / Renoir), 1:1 nach Linux

**Stand:** 2026-10-24 · Kernel 0.682.0 · Modul `gpu_amd_renoir` 0.36.0 ·
Quelle `~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55[-renoir]`.

**Prinzip (nicht verhandelbar):** DC wird **ganz** portiert, 1:1 aus Linux'
eigenem Code, wie der Rest des Moduls (`amdgpu_dm.rs`, `gmc_v9_0.rs`, … mit
Zeilen-Referenzen im Kopf und Orakel-Gegenprobe). **Kein Flip-only-Abkürzungs-
pfad**, der DML, Clock-Manager oder die hwss-Sequenzen durch Schätzungen
ersetzt — genau diese Teile entscheiden, ob ein Bild reißt oder unterläuft.
Features, die Renoirs eDP-Alltag nicht braucht (DSC, MST, HDCP, PSR/ABM,
externe Monitore, Audio), sind **spätere Stufen, keine Stubs**: der Kernpfad
wird vollständig gebaut, die Features kommen als eigene faithful Ports dazu.

## Ist-Stand

Portiert: `amdgpu_dm.rs` (dm_early_init / dm_sw_init / dm_dmub_sw_init) +
`dmub_srv.rs` (Regionen) — also bis zu den DMUB-Framebuffer-Regionen. **Ab
`dm_hw_init` / `dc_create` fehlt alles.** `dc_enabled=true` steht, aber es gibt
keine `dc`-Instanz, keinen Resource-Pool, keine hwss, kein DML, keinen
Clock-Manager, keinen Commit-Pfad. Heute geht jeder Gast-Frame im Wirt über die
Textureinheit in einen linearen Puffer (eine GFX-Kopie je Bild); DC ersetzt das
durch einen Scanout/Flip der Anzeige-Engine.

## Umfang (Renoir-Teilpfad, LOC der Linux-.c)

    core/dc.c 6437 · dc_resource.c 5620 · dc_stream.c 1237 · dc_surface.c 312
    · dc_state.c 1077
    resource/ dcn21 1716 + dcn20 2779 + dcn10 1690   (Renoir erbt dcn20/dcn10)
    hwss/     dcn21  453 + dcn20 3407 + dcn10 4271
    clk_mgr/dcn21 (rn_clk_mgr + rn_vbios_smu) 1044
    dml (NUR der dcn21-Teil des 77950-LOC-Baums; dml2 ist dcn32+, NICHT Renoir)
    link/ (nur der Renoir-eDP/DP-Pfad des 18764-LOC-Baums)
    bios/ bios_parser2 (Renoir-Teil des 12792-LOC-Baums)
    IP-Blöcke je dcn10/20/21: hubp · dpp · mpc · opp · optc · dccg · hubbub ·
      dio (stream_encoder) · link_encoder
    dc_dmub_srv.c (Mailbox über die vorhandenen dmub_srv-Regionen) · dc irq

Realistisch **~30–40k LOC** — der mit Abstand größte Block des OS. Mehrere
Sessions. **dml2 und die anderen dcn-Familien sind ausdrücklich draußen.**

## Methode (wie der Rest des Moduls)

Jede Stufe wird host-seitig gegen Linux geprüft, bevor sie aufs Gerät geht
(`hostcheck/check.sh`, `docs/plan/GPU_AMD_LINUX_MAP.md §2a`):

- **Orakel** `oracle/main_dc.c`: `oracle/extract.py` schneidet die DC-Funktionen
  der Stufe wörtlich aus der Linux-Quelle (dc core, dcn21/20/10, dml-dcn21,
  rn_clk_mgr). Verglichen werden **Zeilen / Registerschreibzugriffe / berechnete
  Werte**, Zahl für Zahl.
- **`fake_gpu.rs` erweitern:** ein DCN-Anzeigemodell hinter den Registern —
  Registerräume für hubp/dpp/mpc/optc/opp/dccg/hubbub/dio, das OTG (Vblank/
  Vupdate als IV im IH-Ring) und die **DMUB-Mailbox** (quittiert Befehle, setzt
  Fences). Das ist Gegenprobe der Abläufe, kein Orakel.
- **`<stufe>_check.rs`** fährt den Port auf der Ersatzkarte und vergleicht gegen
  `main_dc.c`. Die Stufen 1–4 brauchen **kein Gerät** — erst die Commit-/Flip-
  Stufen haben Geräte-Meilensteine (Florian fährt sie).

## Kernel-Seam

- `GpuHal` braucht Flip/Vblank/Cursor-Ops (heute `supports_flip=false`,
  GPU_AMD_NATIVE §5.2). Der Wirt-Compositor reicht einen Gast-Scanout-BO an DC
  statt ihn zu kopieren; DC flippt ihn im Vblank.
- eDP ist beim Boot schon von der Firmware an (wir erben den Bootmodus-
  Framebuffer). **Erststufe: den laufenden eDP ÜBERNEHMEN**, nicht neu trainieren
  — volles Link-Training ist eine spätere Stufe.

## Stufen (nach Abhängigkeit)

- **A — Gerüst + Objektgraph.** `dm_services`/`os_types`-Shim (jede DC-Datei
  inkludiert ihn; auf `npk_sys`-Registerzugriff abgebildet), `dc_types`/
  `dc_hw_types`, dann `dc_create` → `dc_construct` → `dcn21_resource_construct`:
  der volle Objektbaum (Pipes 4, hubp/dpp/opp/optc/mpc/dccg, clk_mgr, dmub,
  Clock-Tabelle). Kompiliert, baut den Baum, **keine HW-Schreibzugriffe**.
  Orakel: Konstruktions-Invarianten (Zähler, Clock-Tabelle, caps) gegen
  `dcn21_resource_construct`.
- **B — Clock-Manager + DMUB-Mailbox.** `rn_clk_mgr_construct` + `rn_vbios_smu`
  (Clocks über SMU), `dc_dmub_srv` Befehlsweg auf den vorhandenen Regionen.
  Orakel: die SMU-Clock-Nachrichten + DMUB-Befehle, Wort für Wort.
- **C — DML (dcn21).** `display_mode_vba` + rq/dlg/ttu + die dcn21-soc/ip-
  Parameter. Der große Festkomma-Block. **Hier wohnt „richtig".** Orakel: DML
  auf einer bekannten Mode (eDP 1920×1080 u.a.) → Watermarks/rq/dlg Zahl für
  Zahl gegen Linux' DML.
- **D — hwss init_hw.** `dcn10/20_init_hw` (+dcn21): Power, DCCG/DIO/PHY,
  hubbub/mpc/opp in bekannten Zustand. Orakel: die Registerschreibfolge von
  `*_init_hw`, zeilengleich.
- **E — Commit (statisches Bild).** `dc_validate` + `dc_commit_streams` +
  `dc_update_planes_and_stream` → `program_front_end` (hubp/dpp/mpc/optc/opp),
  `bios_parser2` für Timing. **Geräte-Meilenstein:** ein stehender Framebuffer
  über DC-Scanout statt der Textureinheit-Kopie.
- **F — Vblank + Page-Flip.** dc irq, OTG Vblank/Vupdate, `dc_flip_surface`;
  Kernel-`GpuHal` Flip/Vblank. **Geräte-Meilenstein:** Gast-Frame reißfrei im
  Vblank geflippt, **keine Kopie** — die Scroll-„Bugwelle" (NATIVE §5.2) weg.
- **G — Hardware-Cursor.** dcn10/20-Cursor auf hubp/dpp; Kernel-Cursor-Op.
- **H — eDP-Übernahme.** Den firmware-gelichteten eDP sauber übernehmen
  (Timing/Backlight auslesen, nicht neu trainieren).

**Später, je eigene faithful Stufe (keine Abkürzung):** externe DP/HDMI +
Hotplug (link-Detection/Training), DP/HDMI-Audio, DSC, MST, PSR/ABM, Backlight/
ABM-Feinregelung, S0ix.

## Stufe-A-Fortschritt (2026-10-22)

Gebaut (ruhend, keine Aufrufer, nichts released — host-seitig gebaut):
- **`src/dc_helper.rs`** — die Register-Helfer-Schicht (`dm_read_reg`/
  `dm_write_reg` → `adev.rreg32`/`wreg32`; `generic_reg_set/update/get/wait`),
  1:1 aus `dc_helper.c` + `dm_services.h` + `reg_helper.h`. Die variadischen
  `REG_SET`/`REG_UPDATE`-Makros sind explizite `(shift, mask, value)`-Slices.
- **`src/dc_types.rs`** — `dce_environment`, `dce_version` (Renoir =
  `DCN_VERSION_2_1`), `dp_protocol_version`, `dc_versions`, `DC_VER`, **und der
  ganze `dc_caps`-Baum** (dc_plane_cap/dc_color_caps→dpp/mpc→rom_curve/lut3d,
  dc_dmub_caps, dc_scl_caps; C-`uns:1` → bool).

Gemessene Fundstücke für den Weiterbau:
- **Register:** kommen sauber über `regs.list` → `gen_regs.py` (liest die
  Linux-asic_reg-Header, emittiert `NAME` + `NAME__FIELD__SHIFT/_MASK` — DCE
  ist schon drin). Für die IP-Blöcke `regs.list` um die `dcn_2_1_0`-Register
  erweitern, dann `dc_helper`-Slices füllen. **Die Pipeline läuft hier
  (gcc + Callgraph-Header), aber LANGSAM — ein voller Lauf >3 min; also je
  Block/Batch einmal im Hintergrund regenerieren, nicht je Register.**
- **`adev` ersetzt `ctx->driver_context`** (kein Global; das Modul reicht
  `&amdgpu_device` durch).
- **Abhängigkeitskette von `dc_create`** (gelesen in `core/dc.c`): schon
  `dc_construct_ctx` braucht `link_create_link_service` (link/-Subsystem),
  `dc_perf_trace_create` und später `dc_bios`/`clk_mgr`/`resource_pool`. Der
  erste host-prüfbare Meilenstein liegt also hinter diesen — der Objektgraph
  muss weitgehend stehen, bevor viel läuft (wie im Prinzip oben benannt).

Weiter gebaut (ruhend):
- **Spine** `dc.rs` (`dc`, `dc_context`, `dc_init_data`), `dc_resource.rs`
  (`resource_pool` grow-as-reached, `NO_UNDERLAY_PIPE`), `dcn21_resource.rs`
  (`RES_CAP_RN`, `dcn21_set_caps`, `dcn21_resource_construct`-Kopf).
- **DML-Bounding-Box** `dml_structs.rs` (`_vcs_dpi_ip_params_st`,
  `_vcs_dpi_soc_bounding_box_st` + `_vcs_dpi_voltage_scaling_st` + Enums),
  `dcn20_fpu.rs` (`dcn2_1_ip()`, `dcn2_1_soc()` — Renoirs Werte 1:1). Das sind
  die DML-Eingaben; das formale Orakel kommt mit der DML-Verdrahtung.

### Stufe-A-Fortschritt (2026-10-23): Clock Sources

Erster „Create resources"-Block portiert (ruhend, baut, nichts released):
`src/dce_clock_source.rs` — `clock_source_id`/`clock_source`, `dce110_clk_src`
+ `_regs`/`_shift`/`_mask`, die Statics `CLK_SRC_REGS[5]`/`CS_SHIFT`/`CS_MASK`
(1:1 aus `CS_COMMON_REG_LIST_DCN2_1` + `CS_COMMON_MASK_SH_LIST_DCN2_0`) und
`dce112_clk_src_construct`→`dcn20_clk_src_construct`→`dcn21_clock_source_create`.
**Renoir nimmt den kurzen `dce112`-Pfad**: der Konstruktor legt nur Zeiger +
funcs-Tabelle + `ext_clk_khz` an und **schreibt kein Register** (kein calc_pll /
Spread-Spectrum — das wäre `dce110_clk_src_construct`). Substanz ist also die
Registertabelle. `regs.list` +25 (17 absolute Offsets `BASE(mmX_BASE_IDX)+mmX`
über `dcn21_resource.c`, 4 Shift + 4 Mask), `regs.rs` neu generiert.
`dcn21_resource_construct` legt jetzt die 5 PLLs + DP-DTO an und setzt
`clk_src_count`. Mitgewachsen: `dc_bios`/`dc_firmware_info`/`MAX_PIPES`
(dc_types), `ctx.dc_bios` (dc.rs), `clock_sources[7]`/`dp_clock_source`
(resource_pool). Verifikation: Werte maschinell aus den Linux-Headern +
Makro-Struktur geprüft; das formale Orakel kommt mit dem Objektgraph.

Zweiter Block, **dccg** (`src/dccg.rs`): `dccg`/`dcn_dccg`,
`dccg_registers`/`dccg_shift`/`dccg_mask` als erreichter Teil
(`DCCG_COMMON_REG_LIST_DCN_BASE` + `DCCG_MASK_SH_LIST_DCN2_1`), die Statics
`DCCG_REGS`/`DCCG_SHIFT`/`DCCG_MASK`, `dccg21_create` (Zeiger + funcs, kein
Registerschreib). 8 Offsets + 64 Shift/Mask in `regs.list`; `resource_pool.dccg`
+ der Aufruf im Construct. DCN3+-Taktbäume (HDMI/PHY/DTB/DSC/SYMCLK) bewusst
draußen (grow-as-reached).

Dritter Block, **IPP** (`src/dcn10_ipp.rs`): `input_pixel_processor`/`dcn10_ipp`,
`dcn10_ipp_registers` (14) + `dcn10_ipp_shift`/`_mask` (26, `IPP_DCN10_REG_FIELD_
LIST`), Statics `IPP_REGS[4]`/`IPP_SHIFT`/`IPP_MASK` (`IPP_REG_LIST_DCN20` +
`IPP_MASK_SH_LIST_DCN20`; `OUTPUT_FP` DCN10-only → 0), `dcn20_ipp_construct` +
`dcn21_ipp_create`. `resource_pool.ipps[MAX_PIPES]` + der Pipe-Loop im Construct
(nur ipps; `read_pipe_fuses` fehlt → hw-Instanz = Index). regs.list +52 Offsets
(13×4) +50 Shift/Mask. Schreibt kein Register.

Vierter Block, **HUBP** (`src/dcn21_hubp.rs`), der größte: 111 Register × 4
Pipes + 191 Shift/Mask-Felder. Wegen der Größe NICHT von Hand — das neue
Werkzeug **`dcgen.py`** expandiert Linux' `HUBP_REG_LIST_DCN21` /
`HUBP_MASK_SH_LIST_DCN21` mit dem C-Präprozessor und emittiert die `regs.list`-
Zeilen (dedupt gegen vorhandene Namen) und die Rust-Structs + Statics
(`dcn_hubp2_registers`/`_shift`/`_mask`, `HUBP_REGS[4]`/`HUBP_SHIFT`/`HUBP_MASK`).
Der Kopf (base-struct, funcs-Identität, `hubp21_construct` [Zeiger + inst +
opp/mpcc=0xf], `dcn21_hubp_create`) ist handgeschrieben. `resource_pool.hubps`
+ im Pipe-Loop. Schreibt kein Register. Dazu wurde `gen_regs.py` mehrkernig
(~5× schneller, byte-identisch).

Fünfter Block, **DPP** (`src/dcn20_dpp.rs`, Renoir nutzt den dcn20-DPP), der
größte: 224 Register × 4 + 540 Felder, via `dcgen.py` (Reg-Liste =
`TF_REG_LIST_DCN20`+`_COMMON_APPEND`, Shift/Mask = Haupt- + Debug-Liste — dafür
nimmt `dcgen.py` jetzt Listen von Makro-Aufrufen). Kopf handgeschrieben (base,
`dpp_caps`/`DCN20_DPP_CAP`, LB-Geometrie, `dpp2_construct`, `dcn21_dpp_create`).

Sechster Block, **OPP** (`src/dcn20_opp.rs`, dcn20-OPP): 22 Register × 6
Instanzen (`opp_regs[6]`, genutzt 0..3) + 52 Felder, via `dcgen.py`.

Siebter Block, **OPTC/OTG** (`src/dcn20_optc.rs`, dcn20-OTG): 77 Register × 4 +
170 Felder, via `dcgen.py` (dafür injiziert `dcgen.py` jetzt das bare `SF`-Feld-
makro und dedupt `macros_h` nach Namen). `dcn20_timing_generator_init` liest die
Maske (`max_h_total = tg_mask.OTG_H_TOTAL + 1`). **Damit ist die volle Pipe-Kette
hubp→ipp→dpp→opp→optc portiert.**

Achter Block, **MPC** (`src/dcn20_mpc.rs`, dcn20-MPC): ein Struct mit Array-
Feldern (55 Felder + 2 skalar) + 69 Felder, via `dcgen.py` **single-Modus** (neu:
`gen_single` für Register-Structs, die `SRII` in `.F[inst]` EINES Structs
füllen). Kopf portiert `mpcc`/`mpc2_init_mpcc` (Mischknoten-Reset).

**Nächste Einheit:** `dcn21_hubbub_create` (wie MPC single-Struct),
`dcn21_dsc_create` (array-of-structs je num_dsc=3), dann die Nicht-Tabellen-
Blöcke `resource_construct` (stream/link-enc/audio) +
`dcn21_hw_sequencer_construct` + `dml_init_instance` (Hand-Port). Dazu
`read_pipe_fuses`
(kompaktiert ausgefuste Pipes). Danach aux/i2c je ddc, mpc, hubbub, dsc, dwbc,
mmhubbub, irq-Service, `resource_construct` + `dcn21_hw_sequencer_construct`,
`dml_init_instance`. Die Konstruktoren schreiben KEIN Register (Zeiger +
funcs); die Schreibzugriffe kommen in hwss (Stufe D/E). **Das formale
`main_dc.c`-Orakel (extract.py-Weg + os_types-Shim) wird EINMAL am fertigen
Objektgraph gebaut und prüft dann den ganzen Construct** — der erste
host-prüfbare Meilenstein ist der Graph, nicht der Einzelblock.

## Stufe-E-Fortschritt

- **E1** Stream/Plane/State bis `dc_state_add_plane`, **E2**
  `dc_validate_global_state` + DML (dmlgen.py) — beide bitgleich.
- **E3 `dc_commit_streams`** (2026-10-24): der ganze Weg bis vor
  `dc_update_planes_and_stream`, ruhend. `dc_validate_with_context`,
  `dc_commit_state_no_check`, eDP-Fast-Boot-Prüfung
  (`dc_validate_boot_timing` mit `optc1_get_hw_timing`), Accelerated Mode
  (Backlight aus, Encoder/OTGs/PLLs aus, VGA), `rn_update_clocks` +
  DPP-DTOs, `hubbub21_program_watermarks`, `dce110_apply_ctx_to_hw`
  (OPP-Format, `dcn20_enable_stream_timing`, OTG-Timing, DRR, DIG-FE an
  OTG), `link_set_dpms_on` mit `enable_link_dp` (eDP-Power, OUI,
  8b/10b-Training: Clock Recovery, Channel EQ, Lane-Anpassung über die
  VBIOS-Tabelle, Video-Idle, FEC), Stream-Attribute/MSA, Info-Frames,
  `dcn20_enable_stream`/`unblank_stream`, Front-End-Schritte ohne Plane,
  Stereo über den GPIO-Mux, `dcn20_optimize_bandwidth`, Stream-Maske an den
  DMCUB. Neue Dateien `link_validation.rs`, `link_hwss.rs`,
  `link_dp_training.rs`.
- **Orakel:** `check.sh` vergleicht im Modus `commit` jetzt durch
  `dc_commit_streams` — `boot` 765 Register-Writes + 20 AUX + 25 DMUB-Befehle,
  `lit` (VBIOS bereits beschleunigt, Link aktiv → `disable_link` vorab)
  ebenso, jeweils `full` und `scaled`, Byte-gleich. Die DMUB-Befehle werden
  als Präfix der Linux-Folge verglichen (der Rest gehört zu
  `dc_update_planes_and_stream`).
- **E4 `dc_update_planes_and_stream`** (2026-10-24): die Plane kommt auf
  die Pipe, ruhend. `update_planes_and_stream_v2` mit Update-Typ-Bestimmung
  (`det_surface_update`, Plane-Info/Skalierung), neuem Kontext (Planes ab/an,
  `dcn21_validate_bandwidth`), `commit_planes_for_stream` (alle Pipes
  gesperrt, `dcn20_pipe_control_lock`, Front End, Unlock, Post-Unlock,
  Manual Trigger, Stream-Maske), `dcn20_program_pipe`: Plane-Enable
  (Power-Gating, HUBP-Init, Systemapertur), HUBP (VTG, Requestor, DLG/TTU,
  DEDCN21-142, Viewport, Surface-Config, Flip-Adresse), DPP (CNVC +
  Eingangs-CSC, Skalierer mit Line Buffer, Ratios/Inits und 64-Phasen-Filtern
  aus Linux' Tabellen, Gamut-Remap, Degamma-ROM, HDR-Multiplikator), MPCC
  (Einfügen/Entfernen, Blending, Hintergrund), Ausgangs-CSC, OGAM-Bypass,
  FMT, ABM `set_pipe`/`set_level` über den DMCUB. Neue Dateien
  `dcn10_dpp_dscl.rs`, `dcn20_dpp_cm.rs`, `dce_scl_filters.rs`,
  `conversion.rs`.
- **Orakel:** `check.sh` vergleicht `commit` jetzt bis zum Ende von
  `dc_update_planes_and_stream`, mit ALLEN DMUB-Befehlen — `boot` +350
  Zeilen (31 DMUB-Befehle), `lit` ebenso, `full` und `scaled`, Byte-gleich.
  Zwei Werkzeugfehler dabei behoben: `dcgen.py` ließ die `TF2_SF`/`DSC2_SF`-
  Felder weg (`LB_DATA_FORMAT__ALPHA_EN`, `FORMAT_CONTROL__ALPHA_EN`, zwei
  DSC-Felder), `gen_regs.py` verlor Tabellen, deren Getter GCC mit einer
  gleichen Funktion zusammenfaltete (`-fno-ipa-icf`). Und der Orakel-Shim
  hatte `ilog2(0)` undefiniert statt `fls(0) - 1 = -1` wie der Kernel.
- **Benannt, nicht portiert:** MST, DSC, HDMI/DVI/LVDS-Attribute und
  -Info-Frames, Audio-Ausgang, LTTPR im nicht-transparenten Modus, DPIA,
  Fallback auf niedrigere Link-Settings; Fast-/Medium-Update auf dem
  aktuellen Zustand (der Flip-Pfad), `commit_planes_do_stream_update`,
  Farbmanagement mit Kurven (Degamma-/Shaper-/3D-LUT-/Blend-/OGAM-RAM),
  Cursor, GSL-Gruppen (Immediate Flip), Testmuster.
- **Geräte-Meilenstein E (2026-10-24, 0.33.1):** `gpu_amd_renoir.dc live`
  übernimmt das eDP — Firmware-Timing, Plane auf dem Kernel-Framebuffer,
  Degamma/Regamma im Bypass wie amdgpu_dm ohne Farbeigenschaften. Am
  Gerät: Farben richtig, ein kurzes Flackern beim Treiberstart (kein
  Seamless Boot auf DCN 2.1).
- **E5a Flip** (2026-10-24): ein Flip, wie `amdgpu_dm_commit_planes` ihn
  baut (Plane-/Skalierungsinfo unverändert, neue Adresse, Stream-Update nur
  mit dem Stream), ist ein Fast-Update auf `current_state`:
  `update_planes_and_stream_state` gibt für FAST/MED den aktuellen Zustand
  zurück (`update_context::current`), `update_planes_and_stream_v2`
  committet darauf ohne Tausch, `commit_planes_do_stream_update` portiert.
  Orakel: zwei Flips nach dem Plane-Update, `boot`/`lit` × `full` (zwei
  Pipes) / `scaled`, Byte-gleich.
- **E5b Interrupts** (2026-10-24): DC `irq_service.c` + `irq_service_dcn21.c`
  (Tabelle aus Linux über `gen_regs.py`, das jetzt Zeiger in Tabellen als
  `<name>_SYMS` führt), `dc_interrupt_set/ack/to_irq_source`,
  `dc_get_flip_pending_on_otg`; DM `amdgpu_dm_irq.c` (Quellen, Handlerlisten,
  `amdgpu_dm_irq_handler`), `amdgpu_dm_initialize_drm_device` ohne DRM-
  Objekte (CRTCs, Outbox, Link-Detect, `dcn10_register_irq_handlers`),
  `dm_vupdate_high_irq` → `dm_crtc_high_irq_handler` (DCN: VUPDATE_NO_LOCK
  liefert Vblank UND Flip-Abschluss), DRM-Vblank (`drm_vblank.rs`:
  Referenz, Abschalt-Timer 2 Frames, Ereignis). `dm_page_flip` ist der
  Flip-Teil von `amdgpu_dm_commit_planes`. Orakel: Set/Ack-Schreibzugriffe
  und Quellenzuordnung Byte-gleich. **Gerätetest:** nach der Übernahme zwei
  Flips auf den gezeigten Framebuffer (`dc live: flip N completed …`).
- **Benannt, nicht portiert (E5):** Flip-Drosselung auf den Vblank nach
  dem letzten Flip (`amdgpu_display_get_crtc_scanoutpos`), Hardware-
  Vblank-Zähler, HPD-Handler, DMUB-Outbox-Handler, Vblank-Control-Worker
  (PSR/Idle-Optimierung).
- **E5b am Gerät ✅ (0.34.0):** `flip 0 completed after 16845 us at vblank 1`,
  `flip 1 … 16555 us at vblank 2` — ein Frame bei 60,0 Hz.
- **Entscheid (Florian, 2026-10-24):** erst Hardware-Cursor (G), dann der
  Composer auf Flippen (E5c).
- **G Hardware-Cursor** (2026-10-24, Kernel 0.681.0 + Modul 0.35.0):
  `dc_stream_program_cursor_attributes/position` mit `program_cursor_*`,
  `dcn10_cursor_lock` (+ `delay_cursor_until_vupdate`, `optc1_get_position`,
  `mpc1_cursor_lock`), `dcn10_set_cursor_attribute/position/sdr_white_level`,
  `hubp2_cursor_set_attributes`, `hubp1_cursor_set_position`,
  `dpp2_set_cursor_attributes`, `dpp1_set_cursor_position`,
  `resource_can_pipe_disable_cursor`; Orakel: Attribute + vier Positionen
  (innen, Rand rechts unten, links oben mit Hotspot, aus) Byte-gleich, `full`
  (zwei Pipes) und `scaled`. DM: Cursor-Ebene wie amdgpu_dm (Puffer im VRAM,
  `amdgpu_dm_plane_get_cursor_position`, `handle_cursor_update`). Kernel:
  `OP_CURSOR_IMAGE` / `OP_CURSOR_MOVE` mit `CAP_CURSOR` (Bewegungen werden
  zusammengefasst), `shade::cursor` backt nicht mehr ein, wenn das Modul den
  Hardware-Cursor zeigt. Benannt: DMUB-Cursor-Payloads (PSR-SU/Replay).
- **G am Gerät ✅:** Zeiger flüssig, auch über dem Firefox-Gast.
- **GTT-Scanout am Gerät ✅ (0.35.1):** eine halbe Sekunde aus Schatten A
  (GART-Adresse) ohne Unterlauf.
- **E5c Present** (2026-10-24, Kernel 0.682.0 + Modul 0.36.0): die Anzeige
  gehört dem Modul. Es hält drei Anzeigepuffer (Framebuffer + zwei VRAM-
  Puffer); eine Kopie „in den Framebuffer“ (`OP_COPY_RECT`, Ziel 0) landet in
  einem nicht gezeigten Puffer, samt dem, was diesem seit seinem letzten
  Stand fehlt, und DC flippt im nächsten VUPDATE dorthin. Der Auftrag ist
  fertig, wenn die Kopie fertig ist — der Kernel wartet nie auf einen
  Vblank. Gezeigt / wartend / frei: mit drei Puffern ist immer einer frei,
  ein weiterer Auftrag vor dem Flip schließt sich dem wartenden an. Kernel:
  `CAP_PRESENT`, und dann gehen auch die CPU-Kopien des Framebuffers
  (`blit_rect`, `blit_all`, `blit_region`, `blit_char`, `clear`) als
  GPU-Kopie über das Modul. Beim Abgang flippt das Modul zurück auf den
  Framebuffer.
- **Danach (offen):** Composer flippt die Schatten selbst (spart die
  Vollbild-Kopie), Flip-Drosselung, HW-Vblank-Zähler.
- **Früher geplant als E5c:** Kernel-Naht — ein Flip-Auftrag (`OP_FLIP`) vom
  Composer an das Modul, abgeschlossen beim Ereignis; und der Composer
  zeichnet in den hinteren Puffer und flippt (Entscheid offen: Puffer-Alter
  /Schaden, Cursor bis zum Hardware-Cursor G).

## Risiken / offen

- **DML-Umfang:** selbst der dcn21-Teil ist groß und reiner Festkomma-Code;
  eine Mutation fällt nur im Orakel auf. Nicht abkürzen.
- **bios_parser2 vs. das vorhandene atom/atomfirmware:** DC bringt seinen
  EIGENEN BIOS-Parser mit; klären, was wiederverwendbar ist.
- **DMUB-Firmware-Pfad am Gerät** ist bis Stufe B/E ungeprüft (Regionen stehen,
  die Mailbox nicht).
- **Koordination:** zweite AMD-GPU-Session (native context) — Kernelstand
  abgleichen, bevor eine DC-Stufe den Kernel (GpuHal) anfasst.
