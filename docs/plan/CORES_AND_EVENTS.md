# Kerne und Ereignisse — der Umbau vom Takt zum Ereignis

**Stand:** 2026-09-24. Stufe 0 = Kernel 0.410.0, Stufe 1 = 0.411.x (Worker
ohne Takt, HW + QEMU bestaetigt), Stufe 2a = 0.412.0 (Weckgriffe, ein
Wartezustand, `npk_wait`), Stufe 2b/2c = 0.413.0 + wifi_rtl8822ce 0.68.0
(MSI, WLAN per Interrupt), Stufe 2d (erster Teil) = 0.414.0 + bar 0.10.0 +
dock 0.7.0 (Panels abonnieren), Stufe 3a = 0.415.0 (I/O APIC), 3b = 0.416.0 (Eingabe per Interrupt), 3c-1 = 0.417.0 (Shell als Fiber auf
Kern 0). Rest offen.
**Ausloeser:** Kernel 0.408/0.409 (Treiberkern nimmt keine Intents, neue
Arbeit an den leersten Kern) haben das WLAN von 200 auf ~400 Mbit gebracht —
nicht durch schnelleren Code, sondern durch **Platzierung von Hand**. Das ist
das Symptom: es gibt keinen Scheduler, der das selbst tut, und keinen Kern,
der aufwacht, weil etwas passiert ist — nur Kerne, die aufwachen, weil ein
Takt schlaegt.

Ziel dieses Papiers: festhalten, **wie das System heute tickt**, wie es
**danach** funktionieren soll, und welche **Altlasten** dabei verschwinden.
Es ist zugleich die Dokumentation des Endzustands — was hier unter „Zielbild"
steht, wird beim Bauen zur Beschreibung, nicht zur Absicht.

Verwandt: `SCHEDULER_FIBERS.md` (Fiber, grossteils gebaut),
`MICROKERNEL_REFACTOR.md` (Treiber als WASM), Memory
`project_host_device_irq.md` (MSI-X-Grundlage, gebaut).

---

## 1. Ist-Zustand (ausgezaehlt 2026-09-24)

### 1.1 Drei Taktgeber

| Quelle | Kern | Rate | Was dranhaengt |
|---|---|---|---|
| PIT IRQ0 oder LAPIC Vektor 48 | 0 | 100 Hz | `TICKS` (die Systemzeit!), **xHCI-Ereignisring leeren**, **PS/2 abfragen**, Frequenzstatistik |
| LAPIC Vektor 50 je Worker | 1..N | 100 Hz, beim Download 10 kHz (`set_worker_poll_hz`) | nur EOI — weckt den Kern, damit er nachsieht |
| LAPIC Vektor 49 | vCPU-Kern | ~1 kHz | zwingt VMRUN heraus |

„Tickless" gibt es nur als Kuerzung des periodischen Timers
(`arm_worker_wake_in`), nicht als Deadline-Timer.

**Folge:** Tastatur und Maus kommen nicht ueber einen Interrupt, sondern alle
10 ms aus dem Timer-Handler. Neue Arbeit wird bis zu 10 ms spaet abgeholt
(kein Weck-IPI). Ein `npk_sleep(1)` ist nur deshalb 1 ms, weil der Worker
seinen Timer dafuer umprogrammiert.

### 1.2 Geraete pollen — fast alle

| Geraet | MSI-X | Wartet darauf? |
|---|---|---|
| NVMe | programmiert | **nein** — `io_command` dreht bis 5 Mio. Runden unter `NVME`-Sperre |
| virtio-net RX | ja | ja, nur im microVM-Pfad (`park_vcpu_idle`) |
| intel_nic | nein („Polling model") | — |
| xHCI | nein (IMAN.IE gesetzt, kein MSI) | Timer-Drain 100 Hz |
| virtio_blk | nein | Spin |
| PS/2 | IRQ1-Handler existiert, PIC maskiert | Timer-Drain |
| WLAN rtl8822ce / AX200, audio_hda, i2c_hid (GPIO), EC (SCI) | nein | `npk_sleep`-Schleifen im Modul |

**Kein WASM-Modul ruft `npk_irq_*`**, obwohl die ABI seit Juni steht.

Schlafraster der residenten Module: WLAN `sleep_ms(1)` nach 64 leeren
Blicken · audio_hda 4 ms · i2c_hid 5 ms · wifid 4/50 ms · beak 4/16 ms ·
dock/spell/loft/drun/iris/pick/volume 16 ms · bar 300 ms · aml 10 s.
`npk_input_wait` ist kein Fiber-Park, sondern eine HLT-Schleife, die ihren
Kern haelt.

### 1.3 Kern 0 ist ein Sammelbecken

`intent::run_loop` bzw. `read_line_with_tab` rufen in JEDER Runde: Maus
leeren, `shade::poll_render` (ueber `net::poll`), `net::poll` (NIC + TCP-Timer),
`tick_link_and_reconfigure` (DHCP, **DNS `pump_wanted` — blockiert bis 5,5 s**),
`vm_poll_slice` (kooperativ bis 8 × 3 ms), Aktionen, Tasten. Dazu synchron auf
Kern 0: `maybe_idle_gc` (npkFS-GC), **jedes Enter schreibt die History
verschluesselt nach npkFS**, Tab-Vervollstaendigung listet npkFS, App-Start
holt und entschluesselt das Modul, `lock` leitet den Schluessel ab.

Wer eines davon laenger macht, laesst Maus und Bild stehen.

Absicherungen „nur Kern 0": `SESSIONS` (`static mut`), DHCP/DNS/Link,
`poll_render`, die Eingaberinge (Einzelverbraucher), microVM-Aufbau und
-Abbau, `TICKS`.

### 1.4 Der Scheduler ist eine Platzierung

* Neue Arbeit geht in `DEQUES[0]` (Chase-Lev); ein Worker stiehlt sie.
* Ein Fiber bleibt **fuer immer** auf dem Kern, der ihn gestohlen hat —
  `HostState.core_id` ist beim Start gecacht, es gibt keine Migration.
* Worker laufen mit **IF=0**; keine Praeemption. Ein Intent laeuft bis zum
  Ende und blockiert alle Fiber seines Kerns (`pump_peers` lindert).
* Platzierung per Heuristik `least_loaded` + Sonderregel „WLAN-Kern nimmt
  nichts" — beides aus 0.408/0.409.

### 1.5 Grosse Sperren

| Sperre | Problem |
|---|---|
| `HEAP` | eine fuer alle Allokationen aller Kerne, keine Kerncaches |
| `CONNECTIONS` (TCP) | eine fuer alle Sockets, **beim Senden gehalten**; dazu `POLLING`: ein Kern verarbeitet ALLE Rahmen |
| `NVME` | eine Queue, gepollt unter Sperre; `ROOT_MUTEX`/`FS` ueber Geraete-I/O gehalten |
| `COMPOSITOR` | ein ganzes Bild lang gehalten (`CONSOLE`→`COMPOSITOR`→`SCENES`) |
| `SERIAL` | jede `kprintln`-Zeile, Byte fuer Byte am UART — de facto eine grosse Kernelsperre |
| `CORES` | bei JEDEM `current_core_id()` genommen, dazu ein MMIO-Lesen |

### 1.6 Korrektheitsfehler (Stufe 0)

1. **`DEQUES[0]` hat mehrere Erzeuger.** `spawn_inner` schiebt ohne
   Kernpruefung in die Deque von Kern 0 — Chase-Lev erlaubt nur den
   Eigentuemer. `npk_spawn_module`/`npk_open`/`npk_launch`/`npk_pick` laufen
   aber auf Workern. Zwei gleichzeitige Spawns schreiben denselben Platz:
   ein Task geht verloren oder laeuft doppelt.
2. **Seitentabellen ohne Sperre.** `map_page`/`unmap_page` laufen auf
   mehreren Kernen (forge: `memory.grow`, `Code::map`); `get_or_create` auf
   derselben Tabelle von zwei Kernen legt zwei Tabellen an und verliert eine
   Abbildung. Code verschiedener Module teilt sich Seitentabellen (`NEXT_CODE`
   waechst linear).
3. **`UTF8_TAIL` im Timer-ISR.** `poll_keyboard` haelt die Sperre auf Kern 0
   mit IF=1; feuert der Timer genau dann, dreht sich der ISR auf derselben
   Sperre fest — Kern 0 steht.
4. **Kein TLB-Shootdown.** Wer eine Seite entmappt, invalidiert nur den
   eigenen TLB. **Heute durch die Fiber-Bindung entschaerft**: eine Instanz
   wird auf dem Kern gebaut, gewachsen und abgebaut, auf dem sie laeuft, und
   `map_page` invalidiert lokal. Sobald Fiber migrieren (Stufe 4), ist es ein
   Sandbox-Fehler (Rahmen einer toten Instanz, per veraltetem TLB-Eintrag
   sichtbar fuer die naechste im selben Platz). **Tor fuer Stufe 4.**
   Ein IPI-Shootdown geht heute nicht einmal: Worker laufen mit IF=0 und
   wuerden nie quittieren. Der Weg ist deshalb epochenbasiert (§3.6).

---

## 2. Wie Linux es loest

| Thema | Linux | Uebertragbar? |
|---|---|---|
| Takt | `NO_HZ_IDLE`: jeder Kern programmiert seinen LAPIC im **TSC-Deadline-Modus** auf den naechsten hrtimer; im Leerlauf kein Tick. Zeit aus dem TSC (clocksource), nicht aus einem Zaehler | ja, 1:1 |
| Warten | Wait-Queues; alles wartet auf IRQ, Deadline oder Futex | ja — als Fiber-Park |
| Geraete | MSI-X, **eine Queue je Kern** (NVMe, RSS), danach **NAPI**: IRQ aus, bis 64 Pakete abholen, IRQ an | ja |
| Aufgeschoben | softirq, threaded IRQs, per-CPU kworker | als Fiber |
| Scheduler | Runqueue je Kern (EEVDF), Praeemption per Timer, Wake auf leeren Kern + Reschedule-IPI, Lastausgleich | ja, ohne Prioritaetszoo |
| Kern 0 | nach dem Boot nicht besonders; irqbalance | ja |
| Skalierung | per-CPU ueber GS, SLUB-Kerncaches, feine Sperren, RCU, Log als lockfreier Ring | ja |
| Bild | Compositor ist ein Prozess: Damage + Vblank-IRQ + Pageflip | ja, als Fiber |

**Wo wir es besser koennen, weil gruene Wiese:** Aller Anwendungscode ist
WASM, und forge uebersetzt ihn. forge kann an Schleifenkoepfen und
Funktionseingaengen eine **Epochenpruefung** einsetzen (wie wasmtime
„epoch interruption"): ein rechnender Fiber gibt dort selbst ab. Das ist
Zeitscheiben-Praeemption, **ohne** einen Kernel, der an beliebiger Stelle
unterbrechbar sein muss — die Worker duerfen bei IF=0 bleiben, der
Kontextwechsel bleibt kooperativ und billig.

---

## 3. Zielbild

### 3.1 Grundsaetze

1. **Alle Kerne sind gleich.** Kern 0 ist nach dem Boot ein Worker wie jeder
   andere.
2. **Nichts tickt.** Jeder Kern hat genau einen Deadline-Timer, programmiert
   auf die frueheste Deadline seiner Fiber. Ohne Arbeit schlaeft er, bis ein
   IRQ oder IPI kommt.
3. **Ein Warten fuer alles.** `wait(Menge aus {IRQ-Vektor, Deadline,
   Postfach, Wort}, timeout)`. Im Kernel der Fiber-Park, fuer WASM
   `npk_wait`. `npk_sleep`-Schleifen verschwinden aus den Modulen.
4. **Jeder Dienst ist ein Fiber.** Compositor, Eingabe, Netz (NAPI je Karte),
   DHCP/DNS, die Shell — und auch Intents.
5. **Wecken = Postfach des Zielkerns + IPI, falls er schlaeft.**

### 3.2 Zeit

* **Wanduhr = TSC** (invariant TSC, beim Boot kalibriert). `ticks()` wird zu
  einer Ableitung `rdtsc / (freq/100)` fuer Altaufrufer und verschwindet
  schrittweise (223 Rufstellen).
* **Je Kern eine Timer-Queue** (sortiert nach Deadline). Der LAPIC laeuft im
  TSC-Deadline-Modus (`IA32_TSC_DEADLINE`, CPUID.1:ECX[24]); fehlt er, im
  One-Shot-Modus. Kein periodischer Timer mehr, auf keinem Kern.
* Der Timer-Handler tut nichts ausser „Kern aufwecken". Was heute im Tick
  steht (xHCI, PS/2, Statistik), bekommt eigene Weckquellen.
* **Gebaut (Stufe 1, Worker):** `interrupts::halt_until(deadline, cause)`
  ist die EINE Leerlaufstelle eines Workers — sie programmiert den
  one-shot-Timer mit IF=0 und haelt mit `sti; hlt` an, damit ein IRQ
  zwischen Scharfmachen und Anhalten nicht verloren geht. Neue Arbeit:
  `per_core::wake_idle_workers` schickt Vektor 52 an jeden Worker, der
  `IDLE` gesetzt hat; der Worker setzt `IDLE` VOR seiner letzten
  Postfachpruefung (Dekker). Ein Kern mit laufendem Gast behaelt den
  periodischen 1-kHz-VM-Timer (`VM_TIMER_ON`), `halt_until` fasst ihn
  dort nicht an. Kern 0 behaelt seinen Takt bis Stufe 3.
* **Lehre aus 0.411.0:** Wer einen Fiber auf einen FREMDEN Kern legt
  (`fiber::admit` — microVM-AP-vCPUs, fetch/GPU/9p/net-Worker), verliess sich
  darauf, dass der Kern „spaetestens in 10 ms" nachsieht. Ohne Takt schlief
  er weiter: der Gast wartete 10 s je CPU („CPU2 failed to report alive
  state"). Seit 0.411.1 weckt `admit` den Zielkern (`per_core::wake_core`),
  und ein neuer Ready-Fiber zaehlt in der Leerlaufpruefung als faellig.
  **Jeder Weg, der einem fremden Kern Arbeit hinlegt, muss ihn wecken.**

### 3.3 Warten und Wecken

* `FiberState` wird zu **einem** Zustand `Waiting { mask, deadline }` statt
  vier Sonderfaellen (`Sleeping`, `WaitingIrq`, `WaitingKick`, …).
* Weckquellen: IRQ-Vektor (MSI-X-ISR zaehlt, zielt auf den Kern des
  Wartenden), Postfach (MPSC je Fiber), Deadline.
* **Kern-Postfach** (MPSC) fuer neue Fiber und Weckrufe von fremden Kernen;
  ein schlafender Kern bekommt ein IPI.
* WASM: `npk_wait(mask, timeout_ms)` + `npk_irq_*` wie gehabt. `npk_sleep(n)`
  bleibt als Sonderfall `wait({}, n)`.

**Gebaut (Stufe 2a, 0.412.0):** jeder Fiber hat einen **Weckgriff**
(`fiber::Waker`, Tabelle mit 1024 Plaetzen: Signalbits + Kern). `signal(w,
bits)` setzt Bits und weckt den Kern (`wake_core`, IPI falls er schlaeft) —
lockfrei, auch aus dem ISR. `fiber::wait(mask, deadline)` ist der EINE
Wartezustand `Waiting{mask, deadline}`; `Sleeping`, `WaitingIrq`,
`WaitingKick` sind weg. `irq_wait` meldet sich beim Vektor an
(`irq::set_waiter`), `note_fired` signalisiert `SIG_IRQ`; `kick_wait_until`
meldet sich je Kern an (`KICK_WAITER`), `net_kick_bump` signalisiert
`SIG_KICK`. `widgets::push_event` und `wasm::push_app_key` signalisieren
`SIG_EVENT` an die App, die das Fenster/Terminal liest. **`npk_wait(mask,
timeout_ms)`** fuer Module (Bit 1 = Eingabe; < 0 = ohne Frist, 0 = Frist);
`npk_input_wait` (`top`) laeuft darueber. Die Module selbst (`dock`, `bar`, …)
sind noch nicht umgestellt — das ist Stufe 2d, zusammen mit den Quellen, die
Aenderungen MELDEN.

### 3.4 Geraete

* **xHCI ueber MSI-X** → Eingabe-Fiber; Maus und Tastatur kommen per
  Interrupt, nicht per Tick.
* **WLAN (rtl8822ce, AX200) ueber MSI + NAPI**: IRQ → Fiber wacht → Ring
  leeren bis Budget → IRQ wieder scharf. Ersetzt `sleep_ms(1)` und die
  64-Blicke-Spinschleife.
* audio_hda (IOC-IRQ, braucht MSI statt MSI-X), i2c_hid (GPIO-IRQ), EC (SCI).
* **Panels abonnieren statt abfragen** (Florian 2026-09-24). `bar` holt alle
  300 ms `npk_bar_state` (Uhr HH:MM, Arbeitsflaechen, Fenstertitel — unter
  der Compositor-Sperre), Lautstaerke jede Runde, Akku jede 16. Runde,
  Groessen alle paar Runden; `dock` die Fensterliste alle 16 ms. Danach
  wartet `bar` mit `npk_wait` auf: **Deadline zur naechsten vollen Minute**
  (Uhr) · **Compositor-Ereignis** bei Fokus/Titel/Arbeitsflaeche/Groesse ·
  **aml** bei geaendertem Akkustand · **Audio** bei geaenderter Lautstaerke.
  Also rund einmal je Minute statt dreimal je Sekunde, und trotzdem sofort.
  Dafuer braucht jede Quelle einen Weg, eine Aenderung zu MELDEN — das ist
  der eigentliche Bauposten, nicht das Warten.
* **Gebaut (Stufe 2d, erster Teil, 0.414.0):** `kernel/src/notify.rs` —
  Themen `TOPIC_WINDOWS` (Fingerabdruck von Fokus, Arbeitsflaechen und
  Fensterliste nach jedem vollen Bild, `Compositor::shell_fingerprint`),
  `TOPIC_BATTERY` (`battery::report` bei neuem Wert), `TOPIC_VOLUME`
  (`audio::set_volume` bei neuem Wert), `TOPIC_CONFIG` (npkFS schreibt unter
  `sys/config/`). Abonniert wird ueber `npk_wait` mit `WAIT_STATE` 32
  (RENDER). `bar` wartet auf Eingabe | Zustand bis zur naechsten vollen
  Minute, `dock` auf Eingabe | Zustand OHNE Frist — eine veraltete Anzeige
  heisst dann: eine Aenderung, die niemand gemeldet hat.
* **wifid 0.13.0:** `npk_wait(WAIT_WIFI_EVENT, ohne Frist)` statt 4/50 ms.
* **audio_hda 0.4.0:** MSI (`AZX_DCAPS_PRESET_AMD_SB` erlaubt ihn), IOC
  in beiden BDL-Eintraegen, INTCTL = GIE + Strombit, `SD_INT_MASK` in
  SD_CTL beim Start (`snd_hdac_stream_start`); nach jedem Aufwachen SD_STS
  quittieren wie `snd_hdac_bus_handle_stream_irq`. Ein Aufwachen je
  Ringhaelfte (~43 ms) statt alle 4 ms. CIE bleibt aus: Codec-Verben laufen
  gepollt und nur beim Hochfahren.
* **i2c_hid — offen, eigener Posten:** der Touchpad-Interrupt kommt ueber
  den AMD-GPIO-Controller (AMDI0030) und damit ueber den **IOAPIC**, den wir
  nicht programmieren (bisher alles MSI). Dafuer braucht es einen
  IOAPIC-Treiber (Redirection-Table, GSI aus `_CRS`/MADT) — erst dann kann
  i2c_hid auf den GPIO-IRQ warten statt alle 5 ms den Pegel zu lesen.
* **aml — Rechenstoss alle 10 s (gemessen 2026-09-24, `cores`: ein Kern
  92-100 % ohne einen Halt ueber 500 ms).** Jede Runde `heap_reset()` +
  `Namespace::load` der GANZEN DSDT (56 KB AML) + `_BST`/`_BIF` mit vielen
  EC-Zugriffen, die im Kernel aktiv warten (`ec.rs`, `udelay` bis 10 ms je
  Warteschritt). Linux laedt den Namensraum einmal und wertet danach nur
  `_BST` aus. Zu bauen: Namensraum unter einer Heap-Marke behalten, je
  Runde nur `_BST`; spaeter (Stufe 3, IOAPIC) Akkuereignisse per EC-SCI
  statt 10-s-Raster. Nicht vom Umbau verursacht — erst durch die ehrliche
  Messung (0.411.2) sichtbar.
* **wifi_rtl8822ce 0.69.1 — gelernt:** HIMR steht die Runde ueber auf null,
  und bei null setzt der Chip kein HISR-Bit. Eine Sendequittung waehrend der
  Runde weckte nie; 0.69.0 (Deadlines statt 10 ms) liess den Sendering
  volllaufen: Upload 7 Mbit, 503 Zeitueberschreitungen. Jetzt `tx_isr` NACH
  `enable_interrupt`, wie der Blick auf den RX-Ring. Upload wieder 101 Mbit,
  0 Zeitueberschreitungen. **Jede Quelle, deren Ereignis waehrend der
  maskierten Runde verloren gehen kann, braucht die Nachkontrolle nach dem
  Scharfmachen.**
* **microVM-Netz-Datenebene:** der Worker parkt mit `kick_wait(PARK_SAFETY_MS
  = 2)` und wird gemessen NIE per IRQ geweckt (`cores`, QEMU 2026-09-24:
  `irq=0 timeout=504` je Sekunde) — ein versteckter 500-Hz-Takt auf einem
  eigenen Kern. Die Host-NIC muss ihn wecken (virtio-net-MSI-X ist in QEMU
  laut Memory nie aktiv geworden; beim WLAN kommt der Weckruf aus dem
  Treiber-Fiber, der die Rahmen abliefert).
* NVMe: eine I/O-Queue je Kern mit eigenem Vektor — dann braucht es die
  `NVME`-Sperre im Hot-Path nicht mehr, und FS-Sperren muessen nicht ueber
  einem Park gehalten werden (die Blockade von „B-2" im Memory entfaellt).

**Gebaut (Stufe 2b/2c, 0.413.0 + wifi_rtl8822ce 0.68.0):**
`irq::register` faellt auf normales MSI zurueck (`pci::program_msi`); ein
Vektor gehoert seinem Treiber (`HwDriverState::irq_vector`), `npk_irq_arm`/
`npk_irq_wait` nehmen nur den eigenen. `npk_wait` kennt die Treiberbits
`WAIT_IRQ` 2, `WAIT_NET_TX` 4, `WAIT_WIFI_CMD` 8 und fuer wifid
`WAIT_WIFI_EVENT` 16 — jedes an das Recht gebunden, das der passende
`npk_*_poll` prueft. Der RTL8822CE-Treiber portiert `rtw_pci_enable/
disable_interrupt` und `rtw_pci_irq_recognized` (HIMR0/1/3, HISR0/1/3,
Masken aus `rtw_pci_setup`) und parkt im Leerlauf wie `rtw_pci_napi_poll`:
HISR quittieren, HIMR scharf, Ring nachsehen, `npk_wait(IRQ | CMD | TX,
10 ms)`, HIMR aus. Die 64 leeren Blicke und `sleep_ms(1)` bleiben nur fuer
den Fall ohne MSI. **Gemessen am IdeaPad:** Treiberkern 972 → 142
Aufwachungen/s im Leerlauf (mehr als die 100 der Frist allein, also feuert
der MSI), Bandbreite unveraendert. **wifi_rtl8822ce 0.69.0:** die 10-ms-Frist
ist weg — die Pumpe parkt bis zum fruehesten eigenen Termin (Wachhund 2 s,
Bericht 1 s, RX-Stille 5 s, ADDBA-Antwort, Sondierung, offene
Sendequittungen `next_probe_due`, Umsortier-Frist `ro_due_ms`, CSA mit
10 ms), mindestens 1 ms.

**Gebaut (Stufe 3a, 0.415.0): `kernel/src/ioapic.rs`.** MADT-Typ 1 (I/O
APIC) und Typ 2 (Interrupt Source Override), Register wie Linux
(`io_apic_read/write`, hohes Wort zuerst, `__eoi_ioapic_pin`), beim Start
`clear_IO_APIC_pin` fuer jeden Pin — **ausser SMI- und ExtINT-Pins**: der PIT
kann auf manchen Maschinen ueber den alten PIC und einen ExtINT-Pin kommen
(„virtual wire"), bis Stufe 3e den Takt abschafft. `irq::register_gsi` routet
eine Leitung maskiert auf einen Vektor dieses Kerns; eine PEGEL-Leitung
maskiert der ISR (`irq::isr`), `irq::arm` gibt sie frei — Linux'
`IRQF_ONESHOT`. Noch ohne Nutzer; 3b haengt die PS/2-Tastatur daran.

**Gebaut (Stufe 3b, 0.416.0): Eingabe per Interrupt.** Der i8042 kommt
ueber den I/O APIC (ISA IRQ 1, bei aktivem Aux-Port auch 12) auf Vektor 53,
`keyboard::enable_irq` setzt dabei KBDINT/AUXINT wie Linux und leert vorher
den Ausgabepuffer (`i8042_flush`); danach gehoert der i8042 dem Interrupt
(`PS2_IRQ_ACTIVE`), und `read_key` fragt den Port nicht mehr ab. Jeder
xHCI-Controller bekommt MSI-X auf Vektor 54 und `USBCMD.INTE`; der Handler
quittiert wie `xhci_irq` (USBSTS.EINT, IMAN.IP) und leert den Ereignisring.
Beide auf Kern 0. **Der Tick leert bis 3e weiter mit** — beide Wege laufen
im Interrupt auf Kern 0 und verschachteln sich nicht. `enable_irq` steht
hinter dem zweiten `init_mouse`, weil das die Antworten des i8042 selbst
liest.

**Gebaut (Stufe 3c-1, 0.417.0): Kern 0 hat denselben Fiber-Scheduler wie
ein Worker.** `main` legt die Shell (`intent::run_loop`) als Fiber auf Kern 0
(2 MiB Stack wie der Boot-Stack — keine Schutzseite) und geht in
`per_core::core0_loop`. Die Leerlaufstellen der Shell (`core0_idle_tick`,
beide `hlt` in `read_line_with_tab`) parken jetzt den Fiber (`core0_wait`:
bis Eingabe oder zum naechsten Tick); die Eingabe-Interrupts signalisieren
die Shell (`intent::wake_shell`). Kern 0 nimmt weiter keine Arbeit aus dem
Postfach und behaelt seinen Takt. `wake_core` darf jetzt auch Kern 0 wecken.

### 3.5 Kern 0 aufloesen

| heute auf Kern 0 | danach |
|---|---|
| Rendern in jeder Schleifenrunde | Compositor-Fiber: wacht bei Damage oder zur naechsten Bild-Deadline |
| Maus/Tasten aus dem Tick | Eingabe-Fiber am xHCI-IRQ; Ereignisse per Postfach an den Fokus |
| `net::poll` in jeder Runde | NAPI-Fiber je Karte; TCP-Timer als Deadline |
| DHCP/DNS/Link | Netz-Dienst-Fiber |
| Shell-Lesen + Intent inline | Shell-Fiber; Intents als Fiber |
| npkFS-GC, History-Schreiben | Hintergrund-Fiber |

`SESSIONS` und die anderen „nur Kern 0"-Globalen bekommen dabei einen
Eigentuemer-Fiber statt einer Kernannahme.

### 3.6 Skalieren

* **Per-CPU ueber GS-Basis.** Voraussetzung: der SVM-Pfad sichert/laedt die
  Wirts-GS-Basis um VMRUN (heute kein VMSAVE/VMLOAD — der Gast schreibt das
  MSR direkt, weil die MSRPM nichts abfaengt). Bis dahin `current_core_id`
  lockfrei ueber eine APIC-ID-Tabelle (Stufe 0).
* **Heap:** Magazine je Kern vor dem globalen Heap.
* **TCP:** Verbindungstabelle lesend parallel, Sperre je Verbindung; Senden
  nicht unter der Tabellensperre.
* **Compositor:** Szene doppelt gepuffert, gerendert ausserhalb der Sperre.
* **Log:** lockfreier Ring je Kern + Ausgabe-Fiber.
* **Fiber-Migration + Stehlen** — erst nach dem TLB-Tor:
  * `UNMAP_GEN` wird bei jedem Entmappen erhoeht; jeder Kern gleicht an
    seinen Umschaltpunkten ab (CR3 neu laden, wenn sich die Generation
    geaendert hat); **freigegebene Rahmen gehen in Quarantaene**, bis jeder
    Kern die Generation gesehen hat (schlafende Kerne zaehlen als ruhig,
    sie gleichen beim Aufwachen ab). Kein IPI noetig — passt zu IF=0.
* **Epochen-Praeemption in forge.**

### 3.7 Sichtbarkeit: die Prozessliste IST die Fiberliste

Heute fuehrt `process.rs` eine zweite Buchhaltung neben den Fibern, und nur
drei Wege tragen dort ein: WASM-Apps, Terminalfenster, Intents auf einem
Worker. Unsichtbar sind Kern-0-Intents, die microVM samt ihrer vCPU-Fiber
(Florian 2026-09-24: YouTube laeuft mit Ton, `top` zeigt nichts) und alle
Kernel-Fiber (Netz-Datenebene, fetch-, GPU-, 9p-Worker).

Ziel: **jeder Fiber traegt beim Anlegen einen Namen** und ist damit ein
Eintrag — Kern, Zustand (laeuft / wartet worauf: IRQ, Deadline, Postfach),
Rechenzeit, Aufwachungen und deren Ursache. `top` liest diese Liste und
dazu je Kern: Last, Aufwachungen/s nach Ursache (Timer, IRQ, IPI),
IRQs je Geraet, Timer-Modus. Die Abfrage per `npk_sys_info`-Schluessel
(eine Zahl je Aufruf) wird durch eine Momentaufnahme in einem Zug ersetzt.
Gebaut mit Stufe 2 (der Wartezustand ist dann einer und benennbar).

### 3.8 Parallel rechnen

`par_for(n, f)` ueber die Kerne (Fork-Join mit Postfaechern): Rastern in
Streifen, BLAKE3 als Baum, AES-GCM je Block-Bereich.

---

## 4. Stufen

Jede Stufe ist einzeln messbar (`cores`: Aufwachungen/s + Last je Kern,
`netbench`, `wlan`) und einzeln zurueckrollbar.

| # | Stufe | Messung / Tor |
|---|---|---|
| **0** | Korrektheit: Kern-Postfach statt `DEQUES[0]`, Seitentabellensperre, `UTF8_TAIL`, `current_core_id` lockfrei | Bootet, Apps starten (dock+bar+loft+spell mehrfach), `cores` unveraendert |
| **1** | Zeit: TSC-Wanduhr, Worker-Timer one-shot auf die naechste Deadline (TSC-Deadline-Modus, sonst Zaehlmodus), Weck-IPI fuer neue Arbeit. Kern 0 behaelt seinen Takt bis Stufe 3 | Leerlauf-Aufwachungen/s je Worker ≈ 0 statt 100; Apps starten ohne 10-ms-Verzug |
| **2** | Warten/Wecken: `Waiting{mask}`, IPI-Wake, `npk_wait`; **WLAN auf MSI+NAPI**, dann audio_hda, i2c_hid, Panels | WLAN: kein `sleep_ms(1)` mehr, Einbrueche? Durchsatz ≥ 0.67; Ton ohne Knacken |
| **3** | Kern 0 aufloesen; dabei xHCI ueber MSI-X (Eingabe per IRQ statt Tick) und der Takt von Kern 0 faellt | Maus fluessig waehrend DNS/GC/App-Start; Kern 0 ohne Takt |
| **3b** | Strom: **amd-pstate/CPPC** (MSR `CPPC_ENABLE` 0xC00102B1, `CPPC_REQ` 0xC00102B3, EPP je Kern — das AMD-Gegenstueck zu `enable_hwp`, das nur Intel kennt) und **cpuidle mit tiefen C-States** (MWAIT-Hinweise aus `_CST`, Wahl nach erwarteter Schlafdauer wie Linux' `menu`/`teo`). Erst NACH Stufe 3: C6 lohnt nur, wenn ein Kern lange schlaeft | `power`: Package-Watt im Leerlauf; Kern 0 nicht mehr dauerhaft auf Hoechsttakt (Florian 2026-09-24: am IdeaPad ~3,5 GHz gegen ~1,4 GHz der anderen). Ehrlich: RAPL mass Kern 0 schon bei 6 mW, der Grossteil der 21 W liegt ausserhalb der Kerne (`project_idle_power_21w`) |
| **4** | Skalieren: Heap-Magazine, NVMe je Kern, TCP-Sperren, TLB-Epochen, Migration, Epochen-Praeemption | Zwei Downloads parallel skalieren; rechnender Fiber blockiert keinen Nachbarn |
| **5** | `par_for`, paralleles Rastern | Bildzeit bei 4K |

---

## 5. Altlasten, die dabei verschwinden

Beim jeweiligen Schritt verifizieren, nicht blind loeschen.

| Altlast | Stufe | Ersetzt durch |
|---|---|---|
| ~~Chase-Lev `DEQUES` (256 × 256 Plaetze statisch), `spawn_local` (tot), `Priority`~~ | 0 ✓ | gemeinsames Postfach |
| ~~`current_core_id` mit Sperre + Vektorsuche~~ | 0 ✓ | lockfreie Tabelle, spaeter GS |
| ~~`TICKS` als Wanduhr~~ | 1 ✓ | TSC (`ticks()` bleibt als 10-ms-Einheit) |
| PIT-/APIC-100-Hz-Tick auf Kern 0 | 3 | Dienst-Fiber + Deadlines |
| ~~Worker-100-Hz-Timer periodisch, `arm_worker_wake_in`/`restore_worker_reload`~~ | 1 ✓ | `halt_until` + one-shot |
| `set_worker_poll_hz` (jetzt nur noch Weckperiode von `worker_idle_hlt`) | 2 | NAPI |
| xHCI-/PS/2-Drain im Timer-ISR | 3 | MSI-X / eigener IRQ |
| ~~`FiberState::{Sleeping, WaitingIrq, WaitingKick}`~~ | 2a ✓ | `Waiting{mask, deadline}` + Weckgriff |
| ~~`npk_input_wait` als HLT-Schleife~~ | 2a ✓ | Fiber-Park, Taste signalisiert |
| `process.rs` als zweite Buchhaltung neben den Fibern, `npk_sys_info`-Einzelabfragen in `top` | 2 | Fiberliste + Momentaufnahme |
| `npk_sleep`-Pollschleifen in den Modulen | 2 | `npk_wait` + IRQ |
| `pump_peers` (Fiber aus einem Intent heraus pumpen) | 2/3 | Intents als Fiber |
| WLAN-Kern-Sonderregel, `least_loaded`, `NATIVE_BUSY` | 2/4 | Scheduler mit Wake-Platzierung |
| `core0_idle_tick`, die Fokus-Zweige von `run_loop`, `is_core0_intent` | 3 | Dienst-Fiber |
| `net::poll` als Allzweck-Pumpe (inkl. `poll_render`) | 3 | NAPI-Fiber + Compositor-Fiber |
| `POLLING`-Flag, `WASM_NIC_RX`-Ueberlaufring | 4 | NAPI je Karte |
| dedizierter VM-Kern (`DEDICATED_VM_CORE`, A2-Pfad) | 4 | vCPU als gewoehnlicher Fiber (Fibermodus ist Vorgabe) |

---

## 6. Offen

* **GS-Basis unter SVM:** VMSAVE/VMLOAD um VMRUN einfuehren oder die
  Wirts-GS nach jedem #VMEXIT vor `stgi` zurueckschreiben. Muss vor
  per-CPU-ueber-GS entschieden sein.
* **Epochen-Praeemption:** Kosten der Pruefung in forge-Code messen
  (Schleifenkopf = ein Laden + Vergleich).
* **Bild-Deadline:** ohne Vblank-IRQ (GOP/UC-Framebuffer) ist die
  Bild-Deadline eine TSC-Deadline von 16,7 ms; mit intel_xe spaeter Vblank.
