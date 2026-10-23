# GPU_AMD_DC — Display Controller (DCN 2.1 / Renoir), 1:1 nach Linux

**Stand:** 2026-10-22 · Kernel 0.669.0 · Modul `gpu_amd_renoir` 0.30.0 ·
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

**Nächste Einheit:** die restlichen „Create resources"-Konstruktoren, der Reihe
nach — **`dccg21_create` zuerst** (erster Block mit echtem `dccg_regs`-Satz),
dann je Pipe hubp/ipp/dpp/opp/optc, aux/i2c, mpc, hubbub, dsc, dwbc, mmhubbub,
der irq-Service, `resource_construct` + `dcn21_hw_sequencer_construct`, und
`dml_init_instance`. Je Block: `regs.list` erweitern → `gen_regs.py`
(Hintergrund, mehrere Minuten) → Struct + funcs + Konstruktor. Die meisten
Konstruktoren schreiben KEIN Register (Zeiger + funcs-Tabelle); die
Schreibzugriffe kommen in hwss (Stufe D/E). **Das formale `main_dc.c`-Orakel
(extract.py-Weg + os_types-Shim) wird EINMAL am fertigen Objektgraph gebaut und
prüft dann den ganzen Construct** — der erste host-prüfbare Meilenstein ist der
Graph, nicht der Einzelblock.

## Risiken / offen

- **DML-Umfang:** selbst der dcn21-Teil ist groß und reiner Festkomma-Code;
  eine Mutation fällt nur im Orakel auf. Nicht abkürzen.
- **bios_parser2 vs. das vorhandene atom/atomfirmware:** DC bringt seinen
  EIGENEN BIOS-Parser mit; klären, was wiederverwendbar ist.
- **DMUB-Firmware-Pfad am Gerät** ist bis Stufe B/E ungeprüft (Regionen stehen,
  die Mailbox nicht).
- **Koordination:** zweite AMD-GPU-Session (native context) — Kernelstand
  abgleichen, bevor eine DC-Stufe den Kernel (GpuHal) anfasst.
