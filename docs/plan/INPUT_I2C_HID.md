# I2C-HID — der Zeiger, der nicht auf PCI liegt

**Stand 2026-09-19.** Auftrag: das Touchpad des Lenovo IdeaPad Flex 5 14ALC7
(AMD Ryzen 5 5500U, Lucienne/Renoir-FCH) zum Laufen bringen — und zwar so,
dass es auf jeder anderen Maschine genauso funktioniert und auf QEMU/NUC/HP
**nichts** verändert.

## Was gemessen ist, bevor hier eine Zeile steht

1. **PS/2 ist tot.** `0xA9` („test auxiliary interface") sagt am IdeaPad
   `0x3` = Datenleitung haengt auf Low. Das ist nicht „Port leer" (das waere
   `0x00`), sondern „kein funktionierender zweiter Kanal". Der i8042 traegt
   die Tastatur auf Kanal 1, Kanal 2 gibt es nicht. Nicht weiter dort suchen.
2. **Kein I2C-Controller auf PCI.** Florians `pci` listet 30 Geraete, keines
   davon ist ein I2C-Controller. Auf Renoir/Lucienne haengt der Designware-I2C
   an fester MMIO im FCH und wird **nur ueber ACPI deklariert**. Die PCI-Liste
   kann die Frage also gar nicht beantworten.
3. **Es gibt keinen Interrupt-Weg fuer ein Nicht-PCI-Geraet.** `irq.rs`:
   „The host kernel has no IOAPIC and the legacy 8259 PIC is fully masked."
   Geraete-IRQs laufen ausschliesslich ueber **MSI-X**. Der FCH-I2C und der
   AMD-GPIO-Controller haben kein MSI-X. **Pollen ist hier kein Kompromiss,
   sondern der einzige vorhandene Mechanismus** — und deckt sich mit dem
   Rest des Systems (`project_host_device_irq`: jeder Treiber pollt).

   **Und das ist nichts AMD-Eigenes.** Ein Geraet ohne PCI meldet seinen
   Interrupt als **GSI** im `Interrupt()`-Deskriptor seines `_CRS`, und die
   routet der **IOAPIC** — auf Intel genauso. Es gibt also keine AMD-Variante
   nachzubauen; uns fehlt ein *generischer* IOAPIC (die MADT liest `smp::init`
   schon, es fehlt die Redirection-Table). Das ist ein eigener, lohnender
   Posten — der Unterbau fuer `project_host_device_irq` —, **aber fuer das
   Touchpad brachte er allein nichts**: dessen „Daten liegen bereit" ist
   nicht der Interrupt des I2C-Controllers, sondern ein **GPIO-Pin**, und der
   AMD-GPIO-Block buendelt ALLE Pins auf EINE GSI. Mit IOAPIC allein haette
   man „irgendein GPIO hat gemeldet" und braeuchte `pinctrl-amd` zum
   Demultiplexen obendrauf. Denselben Pin kann man stattdessen **lesen** —
   siehe Stufe 3.
4. **`AMDI0010` faehrt mit 150 MHz Eingangstakt.** `drivers/acpi/acpi_apd.c`:
   `{ "AMDI0010", APD_ADDR(wt_i2c_desc) }`, `wt_i2c_desc.fixed_clk_rate =
   150000000`. (`AMD0010` = 133 MHz, `AMDI0019` = ebenfalls 150 MHz.) Genau
   die Konstante, die man sonst raet.
5. **Die Entdeckungsschicht ist host-seitig pruefbar.** `tools/wasm/aml/dev/
   DSDT.aml` (HP, Intel-LPSS-Maschine) enthaelt ein Geraet mit
   `_CID "PNP0C50"`, `_HID` als **Methode** (`Return ("SYNA30A1")`), `_DSM`
   und ein `_CRS`, das `ConcatenateResTemplate (SBFB, SBFI)` zurueckgibt.
   Damit laesst sich Stufe 0 vollstaendig gegen eine ECHTE Tabelle fahren,
   bevor irgendetwas aufs Geraet geht.

## Der Schnitt: was in den Kernel gehoert und was nicht

**Kernel — zwei dumme Primitive, kein Geraetewissen:**

| Host-Call | Warum |
|---|---|
| `npk_mmio_map_phys(addr, len)` | Der Controller liegt nicht auf PCI, `npk_mmio_map_bar` greift nicht. |
| `npk_pointer_inject(dx, dy, buttons, scroll)` | Speist `xhci::inject_mouse` — dieselbe Schlange, aus der PS/2 und USB schon kommen. Kein zweiter Weg in den Compositor. |

**Modul `i2c_hid.wasm` — austauschbar wie `wifi_ax200` / `audio_hda`:** die
ganze Entdeckung, der Bustreiber, das Protokoll und die Berichtsauswertung.

Auf einer Maschine ohne PNP0C50-Geraet startet das Modul und beendet sich mit
einer Zeile. Die zwei Host-Calls beruehrt sonst niemand. **Das ist die
Antwort auf „darf QEMU nicht beeinflussen".**

## Sicherheits-Checkpoint: `npk_mmio_map_phys`

„Kann ein WASM-Modul durch diese Aenderung aus seinem Sandkasten?"

Die ehrliche Antwort ist: **ein Modul mit `Rights::HARDWARE` kann das heute
schon** — `npk_mmio_map_bar` + `npk_dma_alloc` reichen dafuer. Der Zuwachs
ist also begrenzt, aber nicht null, und er braucht dieselben Schranken wie
`npk_acpi_mem_read` (Kernel 0.365.0), plus eine mehr:

* **`Rights::HARDWARE`** — dasselbe Recht wie EC-Ports und DSDT.
* **Niemals Arbeitsspeicher.** `memory::is_usable_ram` lehnt jede Adresse in
  der gemeldeten RAM-Karte ab; darin liegen Kernel, Halde und die linearen
  Speicher aller Module. Kennt der Kernel die Karte nicht, gilt **alles** als
  RAM, also alles als tabu. Geprueft wird **jede Seite** der Spanne, nicht
  nur die erste — eine Spanne, die am Rand eines Lochs beginnt, darf nicht
  in den RAM hineinreichen.
* **Niemals der LAPIC-Bereich** `[0xFEE00000, 0xFEF00000)`. Eine Schreibung
  dorthin ist ein Interrupt an einen beliebigen Vektor auf einem beliebigen
  Kern — das ist Codeausfuehrung im Kernel, nicht Geraetezugriff.
  `irq.rs` sagt selbst, dass MSI-X genau so funktioniert.
* **Deckel auf die Spanne** (64 KiB) und auf die Zahl der Abbildungen, wie
  `MAX_MMIO_MAPS` es fuer BARs tut.

`npk_pointer_inject` braucht ebenfalls ein Recht. **Vorsicht:
`npk_key_inject` prueft heute GAR KEINS** — jedes Modul kann Tastendruecke in
die Shell schreiben. Das ist ein eigener Befund (siehe unten), und es ist
ausdruecklich **nicht** die Vorlage.

## Die Aufrufkette, Funktion fuer Funktion

Regel 9 aus `feedback_linux_strict`: der Plan nennt JEDE Linux-Funktion im
Pfad, in Aufrufreihenfolge, und legt fest, dass sie VOLLSTAENDIG portiert
wird. Quelle: Linux 6.18.26, im Cache unter
`~/.cache/nopeekos/linux-src/linux-6.18.26/`.

### Stufe 0 — Entdeckung (ACPI) · **GEBAUT 2026-09-19, gruen gegen die echte HP-Tabelle**

    i2c-hid: \_SB_.PCI0.I2C1.CPD0 [SYNA30A1,PNP0C50] addr=0x2c speed=400000 Hz
    i2c-hid:   HID descriptor register 0x0020 (_DSM)
    i2c-hid:   GpioInt pin [0] on \_SB_.PCI0.GPI0 [INT34BB] mmio 0x006e0000+0x10000

`tools/wasm/i2c_hid/{core,harness}`; Test `hp_dsdt_finds_the_touchpad`.
**Vier Luecken im AML-Interpreter lagen auf dem Weg, jede hat genau eine
Zeile des Ergebnisses gekostet:**

1. **`_HID` als METHODE** — `find_batteries` sah nur `Node::Name`, und die
   HP-Tabelle schreibt `Method (_HID) { Return ("SYNA30A1") }`. Ohne das war
   das Geraet gar nicht auffindbar.
2. **`ConcatenateResTemplate` (0x84)** fehlte ganz. Jedes `_CRS` eines
   I2C-HID-Geraets setzt seine Vorlage aus Bus- und GPIO-Teil zusammen.
   Portiert aus ACPICA `acpi_ex_concat_template` (exconcat.c).
3. **`_OSI` war kein NAME, nur ein abgefangener Aufruf.** Die Firmware fragt
   `If (CondRefOf (\_OSI, Local0))`, bekam Nein, und `OSYS` blieb auf
   `0x07D0` — worauf ihr `_CRS` nur den Interrupt zurueckgab (11 statt 75
   Bytes). ACPICA legt `_OSI`/`_OS_`/`_REV`/`_GL_` in
   `acpi_ns_root_initialize` als echte Knoten an; jetzt wir auch.
4. **`Create*Field`** — der Posten, der im Memory als „benannt und nicht
   gebaut" stand, lag auf dem kritischen Pfad: das `_INI` des Touchpads
   setzt ueber `CreateWordField` die Adresse seines HID-Deskriptors und die
   GPIO-Pinnummer. Gebaut als `Node::BufferField` + `Place::BufField`, also
   mit dem QUELLPUFFER als `Obj` — eine Kopie haette jeden Schreibzugriff
   ins Leere laufen lassen. Anweisungen auf Scope-Ebene brauchen dafuer
   einen Interpreter; sie werden jetzt mit ihrem Scope aufgehoben und beim
   Anlauf nachgeholt (`Namespace::deferred`), wie ACPICA die Termliste einer
   Tabelle beim Laden AUSFUEHRT.

Dabei aufgefallen und behoben: **ein fehlschlagendes `_INI` war still**
(`let _ = self.call_path(...)`) — es richtet das Geraet ein, und ein
verschluckter Fehler laesst jeden Folgefehler wie eine Eigenheit der
Firmware aussehen.

**Benannt und NICHT gebaut:** *bedingte Deklarationen auf Scope-Ebene*. Die
HP-Tabelle schreibt `Device (I2C1) { If ((SMD1 != One)) { Name (_HID,
"INT34B3") … } }`, und unser Lader ueberspringt `If`/`Else`/`While` dort.
Deshalb meldet diese Intel-Maschine einen Controller ohne Kennung. **Auf den
AMD-Tabellen sind die I2C-Controller unbedingt deklariert**, der Posten
steht also nicht zwischen uns und dem Touchpad — aber er steht.

**Risiko, das benannt gehoert:** `aml_core` ist derselbe Kern, aus dem das
ausgelieferte `aml`-Modul den AKKU liest. Punkt 3 aendert, welchen Pfad eine
Firmware nimmt (sie haelt uns jetzt fuer ein modernes System) — das ist
richtig und das, was Linux tut, aber es ist eine Aenderung an etwas, das
laeuft. Das Tor ist der Host-Test `dragonfly_battery`; er ist gruen. Der
Akku gehoert beim naechsten Geraetelauf trotzdem angesehen.

### Stufe 0 — die Vorlage, Funktion fuer Funktion

Linux findet das Geraet gar nicht selbst: der ACPI-Kern zaehlt die Namespace-
Geraete auf und `i2c_acpi_*` liest ihre Ressourcen. Portiert wird:

| Linux | Datei | Was daraus wird |
|---|---|---|
| `acpi_dev_get_resources` + `acpi_walk_resources` | `drivers/acpi/resource.c` | `crs::walk()` — Dekodierer fuer Ressourcen-Templates (klein + gross). |
| `i2c_acpi_get_i2c_resource` | `i2c-core-acpi.c:56` | Filter auf `SerialBus`-Deskriptor mit `type == I2C`. |
| `i2c_acpi_fill_info` | `i2c-core-acpi.c:102` | Slave-Adresse, `connection_speed`, `resource_source` = **Pfad des Controllers**. |
| `i2c_hid_acpi_probe` / `_match` | `i2c-hid-acpi.c:93,120` | Treffer auf `_HID`/`_CID` ∈ {`ACPI0C50`, `PNP0C50`}, Blockliste `CHPN0001`, `IDEA5002`. |
| `i2c_hid_acpi_get_descriptor` | `i2c-hid-acpi.c:57` | `_DSM` mit GUID `3cdff6f7-4267-4555-ad05-b30a3d8938de`, rev 1, Funktion 1 → **Adresse des HID-Deskriptors**. |
| `acpi_apd_create_device` + `wt_i2c_desc` | `acpi_apd.c:192,116` | `AMDI0010`/`AMDI0019` → 150 MHz; `AMD0010` → 133 MHz. |
| `i2c_dw_acpi_params` (`SSCN`/`FMCN`/`FPCN`/`HSCN`) | `i2c-designware-common.c:278` | Paket aus 3 Integern → `hcnt`, `lcnt`, `sda_hold`. **Zuerst die Firmware fragen, erst dann rechnen.** |
| `i2c_dw_acpi_round_bus_speed` | `common.c:335` | Busfrequenz aus dem `I2cSerialBus`-Deskriptor, auf eine Standardstufe abgerundet. |

**Was unser AML-Kern dafuer dazubekommt** (`tools/wasm/aml/core`):

* `find_devices_by_id(&["PNP0C50", "ACPI0C50"])` — Verallgemeinerung von
  `find_batteries`. **`_HID` kann eine METHODE sein** (die HP-Tabelle macht
  genau das), `find_batteries` sieht heute nur `Node::Name`. Ein Zweig, den
  es nicht gibt, faellt in den, der Null sagt
  ([[feedback_a_kind_with_no_branch_falls_into_one_that_answers_zero]]).
  `_CID` zaehlt genauso, und es kann ein **Package** mehrerer IDs sein.
* `eval(path, args) -> Value` — `call_path` kann das schon (`Arg0..6` stehen
  in `Frame::args`); es fehlt nur die oeffentliche Tuer und ein `Value::Buf`
  als Argument fuer die `_DSM`-GUID.
* `crs.rs` — der Ressourcen-Dekodierer. Gebraucht werden vier Deskriptoren:
  `Memory32Fixed` (0x86), `Interrupt` (0x89), `SerialBus/I2C` (0x8E, Typ 1)
  und `GpioInt` (0x8C). Der Rest wird uebersprungen, nicht geraten.

**Tor fuer Stufe 0:** der Harness findet in der HP-Tabelle das PNP0C50-Geraet,
nennt Controller-Pfad, Slave-Adresse, Busfrequenz und Deskriptor-Adresse.
`cargo test` im aml-Baum, kein Geraet noetig.

### Stufe 1 — Kernel-ABI

`npk_mmio_map_phys` und `npk_pointer_inject` in **beiden** ABI-Wegen
(`wasm.rs`-Linker UND `wasm/forge_glue.rs`: extern fn + Namenstabelle) —
sonst laeuft es unter einem Motor und unter dem anderen nicht
([[feedback_the_second_engine_only_runs_where_the_first_one_called]]).

### Stufe 2 — Designware-I2C (Bustreiber)

| Linux | Datei:Zeile | Anmerkung |
|---|---|---|
| `i2c_dw_validate_speed` | `common.c:199` | |
| `i2c_dw_adjust_bus_speed` | `common.c:361` | ACPI-Wert gegen Vorgabe. |
| `i2c_dw_scl_hcnt` / `i2c_dw_scl_lcnt` | `common.c:417,440` | `DIV_ROUND_CLOSEST_ULL(ic_clk * (t + tf), 1e6) - 3 / - 1`. Fallzeit `tf` = 300 ns, wenn die Firmware schweigt. |
| `i2c_dw_set_timings_master` | `master.c:44` | Standard 4000/4700 ns, Fast 600/1300 ns, Fast+ 260/500 ns. |
| `i2c_dw_set_sda_hold` | `common.c:460` | Nur ab `COMP_VERSION >= 0x3131312A`. |
| `i2c_dw_set_fifo_size` | `common.c:675` | Aus `COMP_PARAM_1`. |
| `__i2c_dw_disable` / `__i2c_dw_enable` | `common.c:510` | Der Abschalt-Tanz samt `ENABLE_STATUS`-Warten. |
| `i2c_dw_init_master` | `master.c:212` | Schreibreihenfolge 1:1, inkl. `DW_IC_SMBUS_INTR_MASK = 0`. |
| `i2c_dw_configure_fifo_master` | `master.c:34` | `TX_TL = depth/2`, `RX_TL = 0`, dann `DW_IC_CON`. |
| `i2c_dw_xfer_init` | `master.c:254` | Auch der „dummy read" auf `ENABLE_STATUS`. |
| `i2c_dw_wait_bus_not_busy` | `common.c:631` | |
| `i2c_dw_xfer_msg` / `i2c_dw_read` | `master.c:432,~560` | Die FIFO-Pumpe. |
| `i2c_dw_read_clear_intrbits` / `i2c_dw_process_transfer` | `master.c:648,709` | **Wir rufen das aus der Schleife statt aus der ISR** — siehe unten. |
| `i2c_dw_handle_tx_abort` | `common.c:652` | Die Abbruchgruende beim Namen nennen; `7B_ADDR_NOACK` ist der Normalfall „da ist nichts". |

**Die eine bewusste Abweichung, und ihr Grund:** Linux fuellt die FIFOs aus
`i2c_dw_isr`. Wir haben fuer dieses Geraet keinen Interrupt (Befund 3), also
laeuft dieselbe Zustandsmaschine aus einer Warteschleife: `read_clear_intrbits`
→ `process_transfer` → `xfer_msg`, bis `STOP_DET` oder Zeitablauf. **Die
Funktionen und ihre Reihenfolge bleiben, nur der Ausloeser ist ein anderer.**
Linux selbst faehrt in `amd_i2c_dw_xfer_quirk` (`master.c:356`) einen
Pollpfad, es ist also keine erfundene Bauweise. `AMD_UCSI_INTR_REG` daraus ist
**nicht** unser Fall (NAVI-GPU/UCSI) und wird nicht mitkopiert.

### Stufe 3 — HID over I2C (Protokoll)

| Linux | Datei:Zeile |
|---|---|
| `i2c_hid_probe_address` | `i2c-hid-core.c:176` — ein Byte lesen, bei Fehler 400 µs warten und wiederholen. |
| `i2c_hid_fetch_hid_descriptor` | `:893` — 30 Bytes ab der `_DSM`-Adresse; `bcdVersion == 0x0100` und `wHIDDescLength == 30` sind Pflicht. |
| `i2c_hid_encode_command` | `:240` |
| `i2c_hid_set_power(PWR_ON)` | `:418` — **inkl. der 60 ms danach**, die nicht in der Spezifikation stehen, sondern gemessen sind. |
| `i2c_hid_start_hwreset` / `i2c_hid_finish_hwreset` | `:456,500` — RESET, dann auf die Null-Laenge warten (bei uns: pollen statt `wait_event`), dann erneut `PWR_ON`. |
| `i2c_hid_read_register` (Report-Deskriptor) | `:230` |
| `i2c_hid_get_input` | `:524` — **genau hier haengt das Pollen**: lesen, `ret_size == 0` heisst „nichts / Reset fertig", `0xFFFF` ist ein bekannter Muell-Fall. |

**Wonach gepollt wird, und warum nicht blind.** Linux ruft `i2c_hid_get_input`
aus der ISR des GPIO-Interrupts. Wir haben keinen Interrupt — aber wir koennen
denselben Pin **lesen**: im AMD-GPIO-Block ist jeder Pin ein 32-Bit-Register,
**Bit 16 = `PIN_STS`** (`drivers/pinctrl/pinctrl-amd.c`). Ein MMIO-Lesezugriff
je Runde, ~100 ns; die I2C-Transaktion laeuft nur, wenn wirklich etwas anliegt.
Beide Angaben fallen bei Stufe 0 ohnehin ab: die MMIO-Basis aus dem `_CRS` von
`AMDI0030`, die Pin-Nummer aus dem `GpioInt`-Deskriptor des Touchpads.

Das spart nicht nur Strom — es umgeht auch die Frage, ob ein Geraet beim Lesen
ohne Daten die Leitung **dehnt**, statt Laenge 0 zu melden. Findet sich kein
GPIO-Block, wird blind gepollt; das ist dann die Reserve und steht als solche
im Log.

> **Gebaut in 0.23.0, und der Grund war messbar.** 0.14–0.22 pollten blind,
> und Florian sah im Betrieb **fast durchgehend 60 % eines Kerns**. Die
> Rechnung dahinter: ein Leseversuch holt `wMaxInputLength`, bei uns bis
> zu 64 Bytes (der Puffer deckelt dort) — bei 400 kHz **1,4 ms**, in denen
> `dw_i2c::xfer` den Kern gegen die Uhr dreht. Zweimal je Runde, alle 5 ms:
> **~2 von 5 ms sind Busarbeit fuer nichts.** Die genaue Laenge je Geraet
> steht in seiner `hid: … max N`-Zeile im Bootlog.
>
> `i2c_hid_core::gpio` rechnet jetzt `base + pin * 4` und liest Bit 16
> (`PIN_STS`), beides aus `pinctrl-amd.{c,h}`; die Polaritaet kommt aus den
> `int_flags` des `GpioInt` (ACPI Bits 2:1), nicht aus einer Annahme.
> Angefasst wird nur ein Block, dessen Kennung Linux' `amd_gpio_acpi_match`
> auch nimmt — Florians HP fuehrt einen **Intel**-Block (`INT34BB`) an
> derselben Stelle, und ein geratenes Bit 16 waere dort schlimmer als gar
> keine Abfrage.
>
> **Das Tor kann sich nur selbst abschalten, nie den Zeiger.** Drei Wege
> hinaus: ein Pinregister aus lauter Einsen gilt als „liegt an" (dort hat
> niemand geantwortet); eine FLANKE wird gar nicht erst gefragt; und alle
> 100 ms wird trotzdem gelesen. **Schweigen beweist nichts** — ein ruhendes
> Touchpad sagt genauso wenig wie ein kaputtes Tor. Was etwas beweist, ist
> der Widerspruch: kommt drei Gegenproben hintereinander ein Bericht,
> obwohl der Pin nein sagte, geht es aus und wir sind wieder da, wo 0.22.0
> war — mit einer Zeile im Log. Eine richtige Ansage loescht die Zaehlung,
> damit ein Wettlauf (Finger setzt zwischen Pinlesung und Uebertragung auf)
> nicht als Fehler zaehlt, und streckt die Gegenprobe auf eine Sekunde.
>
> **Offen und benannt:** waehrend einer Uebertragung dreht `dw_i2c::xfer`
> weiter gegen die Uhr (`udelay(10)`), statt abzugeben. Das kostet jetzt
> nur noch, solange wirklich Finger auf dem Pad liegen — abgeben ginge erst,
> wenn feststeht, dass in der Zeit kein RX-FIFO ueberlaeuft (Tiefe 32, bei
> 400 kHz 720 us).
| `i2c_hid_get_report` / `set_or_send_report` | `:257,344` — noetig fuer den Praezisions-Modus (Feature-Report 0x07, „Input Mode"). |

### Stufe 4 — Berichte auswerten

Ein Praezisions-Touchpad (Windows Precision Touchpad) meldet Kontaktpunkte,
keine Deltas. Gebraucht wird ein **Report-Deskriptor-Parser** — geraten wird
hier nichts, sonst ist es ein Treiber fuer genau ein Modell.

* Port von `hid_open_report` / `hid_parser_main` / `hid_parser_global` /
  `hid_parser_local` / `hid_add_field` (`drivers/hid/hid-core.c`).
* Gesucht werden Usage Page `0x0D` (Digitizer) mit `X`/`Y`/`Tip Switch`/
  `Contact Count`/`Contact ID` und Button-Page `0x09`.
* **Absolut → relativ** rechnet der Treiber: erster Kontakt setzt den
  Bezugspunkt, danach Differenz. `MouseEvent.dx/dy` sind `i8` — eine schnelle
  Bewegung ueberlaeuft das, also in Schritte zerlegen oder das Feld
  verbreitern (interner Typ, kein Wire-ABI).
* Feature-Report „Input Mode" = 3 schaltet das Pad ueberhaupt erst in den
  Mehrkontakt-Modus; ohne ihn sendet es Maus-Emulation. Beides bedienen.

## Reihenfolge, und was jede Stufe beweist

0. Entdeckung + Harness-Test → **beweist die ACPI-Haelfte ohne Geraet.**
1. Kernel-ABI → **beweist, dass QEMU unveraendert laeuft** (`build.sh qemu`).
2. Bustreiber → erste Geraetezeile: `COMP_TYPE == 0x44570140` liest sich nur,
   wenn Abbildung und Adresse stimmen. **Das ist der Lebenszeichen-Test.**
3. Protokoll → HID-Deskriptor mit plausibler `wVendorID`.
4. Berichte → der Zeiger bewegt sich.

## Offen und benannt

* **Ein IOAPIC bleibt trotzdem der richtige naechste Kernel-Posten** — fuer
  jedes andere Geraet mit GSI, nicht fuer dieses. Hier nur benannt.
* **`npk_key_inject` prueft kein Recht.** Jedes Modul kann Tastendruecke in
  die Shell schreiben. Eigener Posten, hier nur benannt.
* **Der zweite Zeiger-Faden bleibt offen:** die USB-Maus wird vom
  RTL8153-Scan erschlagen, weil `init`, `init_mouse` und `nic_attach` sich den
  xHCI-Controller je selbst hochfahren. Reiner Kernel, betrifft auch QEMU,
  unabhaengig von diesem Papier.
