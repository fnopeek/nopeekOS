# gpu_amd_renoir Stufe 5b — Szenen auf der GPU

**Stand:** 2026-10-20 · begonnen · Vorgaenger: Stufe 5a (Kopien auf SDMA) ✅ Geraet
**Regel (Florian):** keine Zwischenloesungen, keine Kompromisse — direkt so,
wie es am Ende sein soll.

## Ziel

`CAP_SCENE` wie bei Intel: der Compositor schickt SCN1-Szenen
(`GPU_STACK.md` §3.3), das Modul setzt sie auf der GFX-Engine zusammen, Bit
fuer Bit gleich dem CPU-Ausfuehrer (`kernel/src/shade/scene.rs`). Der Kernel
prueft das selbst am ersten ganzen Bild (`shade/gpu_scene.rs` `verify`/`judge`)
und schaltet bei der kleinsten Abweichung auf die CPU zurueck.

## Bausteine

1. **Gemeinsame Shader** — `tools/wasm/gpu_shaders/*.comp`, eine Quelle fuer
   Intel und AMD. Die Gruppenbreite ist Spezialisierungskonstante 0 (Intel
   16 = SIMD16, AMD 64 = wave64, eine Kachelbreite); ein Element-Block bleibt
   16 Woerter (Lane k laedt Wort k % 16). Intel-Code danach Byte fuer Byte
   gleich (`gen_shaders.py --check`). ✅
2. **Werkzeugkette AMD** — Mesa 26.2.4 (`~/.cache/nopeekos/gpu-shaders/build-amd`,
   `-Dvulkan-drivers=amd -Damd-use-llvm=false -Dtools=drm-shim`). `shaders/acoc.c`
   faehrt RADVs echten Compute-Pipeline-Weg hinter dem amdgpu-drm-shim
   (`AMDGPU_GPU_ID=renoir`) und nimmt: Programm, `radv_shader_regs`, User-SGPR-
   Belegung der Push-Konstanten — und zeichnet auf, was RADV in den
   Kommandostrom schreibt: Queue-Praeambel, Dispatch, Barriere + Dispatch,
   Ende. `shaders/gen_shaders.py` schreibt `src/shaders_gen.rs`; das Modul
   sendet genau diese Paketfolge, der Host-Check haelt sie Wort fuer Wort
   gegen RADVs Aufzeichnung (wie `packcheck.c` bei Intel).
3. **`drm_sched`** — Linux reicht Seitentabellen-Updates und Kommandos ueber
   den DRM-Scheduler ein (Entity, Abhaengigkeiten, `run_job` →
   `amdgpu_job_run` → `amdgpu_ib_schedule`, `free_job`). Portiert aus
   `drivers/gpu/drm/scheduler`; das Modul hat keine Threads, also treibt die
   Dienstschleife, was in Linux der Scheduler-Thread tut. Damit bleibt es
   asynchron wie in Linux: eine Szene wartet GPU-seitig auf ihr VM-Update. ✅
   Mit dabei, weil Linux sie schon fuer Kernel-Jobs auf VMID 0 durchlaeuft:
   `dma_fence` (samt `dma_fence_array`), `dma_resv`, `amdgpu_sync`,
   `amdgpu_job` (Gang-Wechsel, Isolation), `amdgpu_ids` (VMID-Vergabe) und
   `amdgpu_vm_flush` mit den Ring-Emits (Pipeline-Sync, TLB-Flush,
   PASID-Abbildung). Die TTM-Kopien laufen seitdem wie in Linux ueber die
   Entity `mman.high_pr`; Host-Check: 0/2048 Pixel falsch.
4. **`amdgpu_vm`** — der Compositor ist ein Client mit eigenem GPU-Adressraum
   wie unter Linux (`amdgpu_vm.c`, `amdgpu_vm_pt.c`, `amdgpu_vm_sdma.c`,
   `amdgpu_ids.c`): Seitentabellen, `amdgpu_vm_bo_map/update`, VMID-Vergabe,
   `amdgpu_vm_flush` beim Einreichen (`gfx_v9_0_ring_emit_vm_flush`). Dasselbe
   Fundament tragen spaeter die nativen Kontexte der Gaeste (Stufe 7). ✅
   Mit `amdgpu_vm_pt` (Cursor, Fragmente, Freigabe nach TLB-Flush),
   `amdgpu_vm_sdma` (PTEPDE/WRITE/COPY), `gmc_v9_0_get_vm_pde/pte`, dem
   Ressourcen-Cursor und TTMs BO-Lebensdauer (Referenzen, geteilte
   `dma_resv`, verzoegertes Loeschen bis die Fences signalisieren).
5. **Einreichen** — wie `amdgpu_cs` mit dem Client-VM: Job mit IB auf einem
   Compute-Ring (RADVs Compute-Queue = MEC-Ring), Fence ueber IH. ✅
   `amdgpu_driver_open_kms` (PASID, `prt_va`, seq64), `amdgpu_ctx`,
   `amdgpu_gem_va_ioctl` und `amdgpu_cs_ioctl` fuer IB-Chunks. Der
   Selbsttest ist ein Client wie unter Linux: zwei GTT-Puffer an hohen
   Adressen, ein IB im einen schreibt ueber den Compute-Ring in den anderen.
   Host-Check: die Fake-GPU uebersetzt VMID > 0 durch die vierstufige
   Tabelle, die SDMA gebaut hat — VM-Flush, PASID-Zeile und IB mit VMID 1
   laufen Paket fuer Paket wie Linux sie schreibt.
6. **Gemeinsame Planung** — `plan.rs` und das Lesen/Pruefen der Szene aus
   `gpu_intel_tgl/src/compose.rs` in eine gemeinsame Crate; beide Module
   benutzen sie, die Breite ist Parameter.
7. **Dienst** — `BO_ADD`/`BO_REMOVE` in den Compositor-VM, `OP_SCENE` mit
   `JOB_OPEN` und Nachsehen in der Schleife, `CAP_SCENE` nach Selbsttest.

## Was RADV fuer einen Dispatch schreibt (acoc, Renoir, wave64)

| Teil | Pakete |
|---|---|
| Praeambel je Einreichung (`initial_preamble_cs`) | `COMPUTE_PGM_HI`, `STATIC_THREAD_MGMT_SE0..3`, `CP_COHER_START_DELAY`, `TA_CS_BC_BASE_ADDR(_HI)`, `COMPUTE_START_X/Y/Z`, `ACQUIRE_MEM` (I/K/V-Cache, L2), `COMPUTE_PIPELINESTAT_ENABLE`, NOP-Fuellung |
| Pipeline + Dispatch | `PGM_LO`, `RSRC1/2`, `RESOURCE_LIMITS`, `NUM_THREAD_X/Y/Z`, Push-Konstanten inline ab `USER_DATA_2`, `DISPATCH_DIRECT` (Initiator 0x45), `DMA_DATA` (Prefetch des Codes, `radv_emit_compute_prefetch`) |
| Abhaengiger Dispatch | `EVENT_WRITE CS_PARTIAL_FLUSH`, `ACQUIRE_MEM`, `DISPATCH_DIRECT` |
| Ende (`vkEndCommandBuffer`) | `CS_PARTIAL_FLUSH`, `ACQUIRE_MEM` (L2-Writeback), NOP-Fuellung |

Kein Ring-Deskriptor: ohne Scratch und Ringe setzt RADV User-SGPR 0/1 nicht.
