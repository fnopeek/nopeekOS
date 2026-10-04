# `tools/wasm/i2c_hid/wasm/src/lib.rs` @ 5e0102684

## L1-14 · `#![no_std]`

```
//! i2c_hid.wasm — das Touchpad, das nicht auf PCI liegt.
//!
//! Stufe 1: **berichten, was die Firmware sagt.** Das Modul holt die DSDT,
//! sucht darin jedes HID-over-I2C-Geraet und schreibt Controller, Adresse,
//! Busfrequenz, Deskriptor-Register und GPIO-Pin ins Log. Ein Zeiger
//! bewegt sich damit noch nicht — aber erst diese Zeilen sagen, WELCHE
//! Register der Bustreiber danach anfassen muss.
//!
//! Der ganze ACPI-Teil liegt in [`i2c_hid_core`] und ist host-seitig gegen
//! eine echte Firmware-Tabelle geprueft (`hp_dsdt_finds_the_touchpad`).
//!
//! Gelesen wird nur, wenn der Interrupt-Pin sagt, dass etwas anliegt
//! ([`Gate`]) — blind zu lesen kostete 1,4 ms Busarbeit je Versuch und
//! damit einen halben Kern im Leerlauf.
```

## L23-25 · `#[unsafe(link_section = ".npk.caps")]`

```
// Rohzugriff auf Firmware und Hardware: HARDWARE (Bit 0x40). Wer eine
// `.npk.caps`-Sektion schreibt, ERSETZT die Vorgabe und muss READ selbst
// mitnennen, wenn er es behalten will — hier wird keines gebraucht.
```

## L33-34 · `core::arch::wasm32::unreachable()`

```
// Trappen, nicht drehen: der Kernel faengt es ab und sagt es auf dem
// Bildschirm. `loop {}` waere ein stiller Haenger.
```

## L55-66 · `static mut VERBOSE: bool = false;`

```
// ── Diagnosezeilen: gebaut, aber im Normalbetrieb still ──────────────
//
// Jeder Fund an diesem Treiber haengt an einer dieser Zeilen — der rohe
// Deskriptor, die ersten Berichte, die Zehn-Sekunden-Buchfuehrung. Sie
// gehoeren deshalb nicht geloescht, sondern geschaltet. EINMAL beim Start
// gefragt (`npk_sys_info(50)` = Konfigwert `log.drivers`), danach kostet
// es einen Vergleich.
//
// Was NICHT hier haengt: was der Treiber ENTSCHEIDET. Welches Geraet
// gefunden wurde, ob der Praezisionsmodus griff, woran das Tor haengt und
// jeder Fehler — das steht immer im Log, sonst ist ein Geraetelauf ohne
// Aussage.
```

## L70 · `unsafe { core::ptr::addr_of!(VERBOSE).read() }`

```
// SAFETY: ein Faden, ein Lauf.
```

## L74 · `fn dbgln(s: &str) {`

```
/// Wie `logln`, aber nur wenn `set log.drivers 1` gesetzt ist.
```

## L79-82 · `struct HostBus { handle: i32 }`

```
/// Der Hardwarezugang des Bustreibers.
///
/// Der Treiber rechnet, diese Huelle greift zu — deshalb laeuft derselbe
/// Code im Pruefstand gegen einen Mock.
```

## L97-106 · `if us >= 1000 {`

```
// Ab einer Millisekunde ABGEBEN, nicht drehen.
//
// wasmi zaehlt je WASM-Befehl, und `run` gibt einem Modul zehn
// Milliarden davon. Eine Warteschleife gegen die Uhr verbraucht
// sie in Sekunden — der erste Lauf mit lebendem Zeiger endete
// nach zehn Sekunden mit „fuel exhausted". `npk_sleep` gibt an den
// Scheduler ab und kostet EINEN Befehl.
//
// Darunter bleibt das Drehen: `npk_sleep` rechnet in Millisekunden,
// und ein I2C-Zyklus dauert 2,5 us.
```

## L122 · `const HEAP_SIZE: usize = 16 * 1024 * 1024;`

```
// ── Bump-Allokator: der Lauf ist einmalig und ganz voruebergehend ──
```

## L144 · `const SIG_SSDT: i32 = i32::from_le_bytes(*b"SSDT");`

```
/// "SSDT", wie die vier Zeichen im Speicher stehen (little-endian).
```

## L146-147 · `const SIG_PSDT: i32 = i32::from_le_bytes(*b"PSDT");`

```
/// ACPICA laedt ausser SSDT auch PSDT und OSDT in den Namespace
/// (`acpi_tb_load_namespace`). Selten, aber es kostet nichts.
```

## L153-154 · `const SSDT_MAX: usize = 256 * 1024;`

```
/// Platz fuer EINE SSDT auf einmal — der Namespace kopiert heraus, was er
/// braucht, also darf der Puffer danach wieder benutzt werden.
```

## L158-170 · `struct FirmwareAccess;`

```
/// Der Firmware-Zugang des Interpreters.
///
/// Einen Embedded Controller fragt ein I2C-HID-Geraet nicht — aber
/// SystemMemory schon, und das ist hier der Unterschied zwischen „laeuft"
/// und „laeuft nicht": das `_STA` der I2C-Controller liest ein
/// Konfigurationsbyte aus dem NVS-Fenster der Firmware. Ohne diesen Zugang
/// erfindet der Interpreter dort eine 0, und die Firmware schliesst
/// pflichtgemaess auf „abgeschaltet" — gemessen an Florians IdeaPad, wo
/// beide Controller als absent gemeldet wurden, obwohl beide laufen.
///
/// Dasselbe Loch hatte der Akku-Treiber, und es steht dort seit
/// Kernel 0.365.0 offen. `npk_acpi_mem_read` ist nur LESEND und lehnt
/// jede Adresse in der RAM-Karte ab.
```

## L184 · `unsafe {`

```
// SAFETY: ein Faden, ein Lauf — einmal gesetzt, danach nur gelesen.
```

## L196 · `let table = unsafe { core::slice::from_raw_parts(dsdt_ptr as *const u8, len as usize) };`

```
// SAFETY: der Kernel hat genau `len` Bytes hineingeschrieben.
```

## L204-210 · `let ssdt_ptr = core::ptr::addr_of_mut!(SSDT) as *mut u8;`

```
// Und JEDE SSDT dazu.
//
// Eine Firmware verteilt ihre Deklarationen ueber die DSDT und
// beliebig viele SSDTs; sie bilden EINEN Namespace, und Linux laedt
// sie alle. Wer nur die DSDT liest, dem fehlen Namen, die woanders
// stehen — hier die Basis der Region mit den Freigabebits der
// I2C-Controller.
```

## L222 · `let t = unsafe { core::slice::from_raw_parts(ssdt_ptr as *const u8, n as usize) };`

```
// SAFETY: der Kernel hat genau `n` Bytes hineingeschrieben.
```

## L233-236 · `let (seen, taken) = ns.resolve_conditionals(&mut ec);`

```
// Bedingte Deklarationen auf Scope-Ebene aufloesen — ACPICA FUEHRT die
// Termliste beim Laden aus, ein `If` dort ist eine Verzweigung. Daran
// haengt auf diesem Geraet `FRTB`, die Basis der Region mit den
// Freigabebits der I2C-Controller.
```

## L245-247 · `for d in &found {`

```
// Sagt ein `_STA` null, noch einmal MIT SPUR: woraus ist die Null
// entstanden? Nur fuer die Controller, und nur im Zweifelsfall — die
// Spur ist laut.
```

## L264-265 · `logln("[i2c-hid] no HID-over-I2C device declared — idle");`

```
// Das ist eine ANTWORT, keine Panne: eine Maschine ohne Touchpad
// (QEMU, die NUC) sagt genau das.
```

## L283-287 · `let mut buf = [0u8; 64];`

```
// Dauerbetrieb. Ein Treiber kehrt nicht zurueck — er horcht.
//
// 5 ms Abstand: ein Touchpad meldet mit etwa 100-200 Hz, und
// `npk_sleep` gibt dazwischen an den Scheduler ab, kostet also weder
// Kern noch Treibstoff.
```

## L289 · `let mut stat_lines_left = 18u32;`

```
// Drei Minuten Buchfuehrung, dann Ruhe.
```

## L294-305 · `for l in live.iter_mut() {`

```
// Hat der Umschalter in den Praezisionsmodus gegriffen?
//
// NICHT ueber die Zeit. 0.15.0 fragte nach zwei Sekunden „hat das
// Geraet etwas gesagt?" und schaltete sonst zurueck — und ein
// Touchpad, das niemand beruehrt, sagt NICHTS. Der Wachhund lief
// also jedesmal, bevor der erste Finger aufsetzte, und nahm den
// Modus wieder weg. Genau deshalb kam am Geraet nur Bericht 1.
//
// Die Frage, die sich beantworten laesst, ist eine andere: kommen
// Berichte, aber NIE der des Touchpads? Dann hat der Schalter
// nicht gegriffen. Schweigen beweist gar nichts und darf deshalb
// auch nichts ausloesen.
```

## L323-327 · `let (poll_now, gate_said_no) = gate_check(l);`

```
// Erst den PIN fragen, dann den Bus anfassen.
//
// Ein Leseversuch holt `wMaxInputLength` Bytes — bis zu 64,
// bei 400 kHz also 1,4 ms auf dem Bus. Der Pin kostet ein
// Register.
```

## L330-331 · `l.skips += 1;`

```
// Wer nicht gefragt wurde, kann nicht schweigen — es gilt
// der letzte echte Befund.
```

## L338-344 · `for i in 0..8 {`

```
// Die Leitung LEER holen, nicht einen Bericht je Runde.
//
// Ein Bild aus zwei Berichten braucht sonst zwei Runden, und
// bei 5 ms Abstand liegt das genau auf der Melderate des
// Geraets — ein Bericht geht verloren, sobald es einmal
// schneller ist als wir. Acht ist der Deckel, damit ein
// schwatzendes Geraet die Runde nicht besetzt.
```

## L353-355 · `if !gate_asserted_now(l) { break; }`

```
// Der Pegel steht, bis der Bericht geholt ist —
// ist er weg, liegt nichts mehr an. Die leere
// Nachlese waere sonst eine ganze Uebertragung.
```

## L372-374 · `let now = { let t = unsafe { npk_now_us() }; if t < 0 { 0 } else { t as u64 } };`

```
// Alle zehn Sekunden sagen, was die Runde wirklich gekostet hat —
// und dann von selbst aufhoeren. Sonst bleibt ein Treiber, der
// einen Kern frisst, eine Ratesache.
```

## L377-381 · `for l in live.iter_mut() {`

```
// Das Antippen drueckt sofort und laesst SPAETER los — sonst
// koennte daraus nie ein Ziehen werden. Der Tritt dafuer steht
// HIER und nicht im Berichtspfad: ein Touchpad, das niemand mehr
// beruehrt, schickt keinen Bericht, und die Taste bliebe unten.
// Genau dieser Fehler ist 0.13.0 ausgeliefert worden.
```

## L403-406 · `use i2c_hid_core::gpio;`

```
// `do_amd_gpio_irq_handler`: every pending pin acknowledged
// (writing back what was read clears its status bits), then
// the EOI to the GPIO unit. The level line the kernel masked
// is released when we wait again.
```

## L408-414 · `let rd = |o: u32| unsafe { npk_mmio_read32(q.handle, o as i32) } as u32;`

```
// ALL pending pins of the block, as `do_amd_gpio_irq_handler`
// does — the line is shared by every pin. 0.28.0 acked only
// ours; a pin the firmware enabled (lid, hotkeys, EC) with
// its status standing kept the level line up for good, and
// the driver span. Ours are acknowledged; any other pending
// pin is not an interrupt anybody here handles, so it is
// masked — Linux: "Disabling spurious GPIO IRQ".
```

## L445-450 · `let now = { let t = unsafe { npk_now_us() }; if t < 0 { 0 } else { t as u64 } };`

```
// **Sleep until the pad reports** (docs/plan/CORES_AND_EVENTS.md).
// It was every 5 ms — 200 wakes a second on an untouched pad.
// Awake early only for our own timers: an open tap releases
// its button after TAP_MS, and the first minutes of stats.
// At most a second: a lost interrupt shows as lag, not as a
// dead pointer.
```

## L470 · `struct IrqMode {`

```
/// The GPIO controller's interrupt, set up for every live pad.
```

## L473 · `block_off: u32,`

```
/// Offset of the GPIO block inside the mapped page.
```

## L475 · `pins: alloc::vec::Vec<u32>,`

```
/// Each pad's pin register (page offset).
```

## L477 · `spurious_logged: u32,`

```
/// How many foreign pending pins were masked (first few are logged).
```

## L481-485 · `fn arm_irq(found: &[i2c_hid_core::discover::HidDevice], live: &[Live]) -> Option<IrqMode> {`

```
/// Wait on the GPIO controller's interrupt instead of polling — or say why
/// not. Needs every live pad gated on a pin of the SAME AMD block, and that
/// block's own line in its `_CRS`. Pin setup as `amd_gpio_irq_set_type`
/// (level, polarity, clear status, the enable-and-wait-for-debounce dance)
/// followed by `amd_gpio_irq_enable` (enable + unmask).
```

## L520-521 · `unsafe { npk_mmio_write32(handle, o, ((cfg | gpio::INTERRUPT_ENABLE) & !gpio::INTERRUPT_MASK) as i32) };`

```
// Enable while still masked, wait for the enable bit to read back
// (the debounce settles), then write the plain configuration.
```

## L527 · `let r = unsafe { npk_mmio_read32(handle, o) } as u32;`

```
// `amd_gpio_irq_enable`.
```

## L541-546 · `fn probe_bus(d: &i2c_hid_core::discover::HidDevice) -> Option<Live> {`

```
/// Den Controller ANFASSEN: abbilden, Kennung lesen, Zaehler rechnen.
///
/// Das ist der erste Schritt, der die Hardware beruehrt — und die Kennung
/// ist die billigste Probe, dass Abbildung und Adresse stimmen. Steht dort
/// `0x44570140` ("DW" + 0x0140), ist der ganze Weg bis hierher richtig:
/// DSDT gelesen, `_CRS` ausgewertet, MMIO abgebildet.
```

## L555-567 · `if !c.present {`

```
// `_STA` sperrt hier NICHT mehr, es warnt nur.
//
// Unser `_STA` wird auf einem Interpreter gerechnet, der zugegebene
// Loecher hat: Operationsregionen ohne hinterlegten Speicher liefern
// eine erfundene 0, und im Log stehen Lesungen an Adressen wie 0x6 —
// das ist ein Fenster, dessen Basis nie berechnet wurde. Eine so
// zustande gekommene 0 ueber einen direkten Hardware-Lesezugriff zu
// stellen, ist die falsche Reihenfolge der Beweise.
//
// Die Grenze liegt deshalb zwischen LESEN und SCHREIBEN: die Kennung
// holen darf man immer (ein Registerlesen im FCH-Bereich antwortet
// schlimmstenfalls mit lauter Einsen), einrichten erst, wenn sie
// stimmt.
```

## L599-600 · `enum Mode {`

```
/// Ein eingerichtetes Geraet, aus dem sich Zeigerbewegung lesen laesst.
/// Wie dieses Geraet seine Zeigerdaten meldet.
```

## L602 · `Mouse {`

```
/// Maus-Nachahmung: ein X, ein Y, Tasten, vielleicht ein Rad.
```

## L608-615 · `Touchpad {`

```
/// Praezisions-Touchpad: KONTAKTPUNKTE. Je Finger ein Tip-Switch, ein
/// X und ein Y — daraus entstehen Gesten, die kein Geraet meldet.
///
/// Die KENNUNG (`Contact Identifier`) faehrt mit, und sie ist kein
/// Beiwerk: liegen zwei Finger auf, muss der Weg aus DEMSELBEN Finger
/// gerechnet werden. Ohne sie ist der Bezugspunkt der „erste Kontakt
/// im Bild", und wenn das Geraet die Reihenfolge einmal tauscht,
/// springt die Strecke um den Fingerabstand.
```

## L622 · `struct Contact {`

```
/// Ein Kontaktplatz im Bericht: liegt er auf, wer ist er, wo ist er.
```

## L630 · `struct Decoder {`

```
/// Ein Bericht und wie er zu lesen ist.
```

## L637 · `struct Live {`

```
/// Ein eingerichtetes Geraet, aus dem sich Zeigerbewegung lesen laesst.
```

## L644-652 · `decoders: alloc::vec::Vec<Decoder>,`

```
/// **Alle** Berichte, die wir lesen koennen — nicht einer.
///
/// 0.14.0 legte sich auf den Touchpad-Bericht fest und warf jeden
/// anderen weg. Greift der Umschalter auf den Praezisionsmodus nicht,
/// sendet das Geraet weiter seinen MAUS-Bericht — und der Zeiger stand
/// still. Linux verteilt eingehende Berichte nach ihrer NUMMER an den
/// passenden Decoder, statt eine Nummer zu erwarten; das ist der
/// Unterschied zwischen „laeuft" und „laeuft, wenn ich richtig
/// geraten habe".
```

## L654 · `unknown_logged: u32,`

```
/// Die ersten paar unbekannten Berichtsnummern melden.
```

## L656-657 · `switched: Option<u8>,`

```
/// Haben wir auf den Praezisionsmodus umgeschaltet, und in welchem
/// Feature-Bericht steht der Schalter?
```

## L659-660 · `mode_len: usize,`

```
/// Wie lang dieser Feature-Bericht ist — die Ruecknahme muss dieselbe
/// Laenge haben wie das Setzen, sonst wird auch sie verworfen.
```

## L662 · `seen: u32,`

```
/// Wieviele Berichte sind bisher gekommen?
```

## L664 · `touch_rid: Option<u8>,`

```
/// Die Nummer des Touchpad-Berichts, falls es einen gibt.
```

## L666 · `saw_touch: bool,`

```
/// Ist er je gekommen? Das ist der BEWEIS, dass der Umschalter griff.
```

## L668 · `other_seen: u32,`

```
/// Wieviele Berichte kamen, die NICHT der des Touchpads sind?
```

## L670-671 · `touch_logged: u32,`

```
/// Wieviele Kontaktlagen wurden schon gemeldet? Die ersten paar
/// gehoeren ins Log: ob ZWEI Finger ankommen, sagt sonst niemand.
```

## L673-674 · `raw_logged: u32,`

```
/// Die ersten Berichte ROH. Was das Geraet wirklich schickt, sagt
/// keine abgeleitete Zahl.
```

## L676 · `scroll_logged: u32,`

```
/// Die ersten Rollentscheidungen.
```

## L678 · `tap_logged: u32,`

```
/// Die ersten Antipper.
```

## L681-685 · `track: i2c_hid_core::gesture::Tracker,`

```
// ── Aus Orten werden Wege und Gesten ─────────────────────────
//
// Das steht in `i2c_hid_core::gesture` und nicht hier, weil genau
// diese Logik zweimal falsch ausgeliefert wurde und beide Male erst
// am Geraet auffiel. Dort haengen Tests daran.
```

## L687 · `have_ref: bool,`

```
/// Bezugspunkt fuer eine Maus, die ORTE statt Wege meldet.
```

## L691-695 · `last_buttons: i32,`

```
/// Die zuletzt gemeldete Tastenlage.
///
/// Ein LOSLASSEN ist ein Ereignis wie ein Druck: wer nur bei
/// `buttons != 0` einspeist, meldet den Druck und nie das Ende — und
/// der Compositor haelt die Taste fuer immer fuer gedrueckt.
```

## L697-703 · `hw_buttons: i32,`

```
/// Die PHYSISCHEN Tasten aus dem letzten Bericht, ohne das, was ein
/// Antippen gerade haelt.
///
/// Beides getrennt zu fuehren ist noetig, weil sie zu verschiedenen
/// Zeiten kommen: die physische Lage steht im Bericht, die gehaltene
/// laeuft an einem Zeitgeber ab — und der tickt auch dann, wenn das
/// Geraet schweigt.
```

## L706 · `gate: Gate,`

```
/// Fragen wir den Pin, bevor wir den Bus anfassen?
```

## L708-712 · `polls: u32,`

```
// ── Was diese zehn Sekunden gekostet haben ───────────────────
//
// Ein Treiber, der 90 % eines Kerns frisst und nichts sagt, laesst
// nur raten. Diese Zeilen sind die Zahlen dazu, und sie hoeren von
// selbst wieder auf.
```

## L720-721 · `err_logged: u32,`

```
/// Die ersten paar Fehlschlaege MIT Grund. Ein stiller Fehlschlag ist
/// der teuerste Zustand ueberhaupt: xfer wartet bis zu einer Sekunde.
```

## L723-727 · `dead: bool,`

```
/// Hat dieses Geraet beim letzten ECHTEN Leseversuch geschwiegen?
///
/// Eine uebersprungene Runde ist kein Schweigen — es wurde gar nicht
/// gefragt. Ohne diesen Merker haette das Tor die Notbremse
/// ausgehebelt: ein toter Bus saehe aus wie ein ruhiges Touchpad.
```

## L731-742 · `enum Gate {`

```
/// Der Pin, der sagt, ob ueberhaupt ein Bericht anliegt.
///
/// **Warum es das gibt.** Ein Leseversuch ist nicht billig: geholt wird
/// `wMaxInputLength`, bei uns bis zu 64 Bytes (der Puffer deckelt dort),
/// und bei 400 kHz sind das 1,4 ms, in denen der Kern auf dem Bus wartet.
/// Zweihundertmal je Sekunde, fuer zwei Geraete. Das ist der Grund, warum
/// dieses Modul im Leerlauf einen halben Kern verbraucht hat.
///
/// Linux liest deshalb NIE blind: `i2c_hid_get_input` haengt dort
/// ausschliesslich an `i2c_hid_irq`. Wir haben keinen Interrupt, aber der
/// Pegel steht an, bis der Bericht geholt ist — also laesst er sich
/// abfragen, und das kostet ein Register statt einer Uebertragung.
```

## L744-745 · `Blind,`

```
/// Kein Pin, kein bekannter Block, oder er hat sich als falsch
/// erwiesen: lesen wie bisher.
```

## L747 · `Pin {`

```
/// Der Pin steht und wird gefragt.
```

## L752 · `skipped: u32,`

```
/// Wieviele Runden hintereinander sagte er „nichts da"?
```

## L754 · `contradictions: u32,`

```
/// Wie oft kam trotzdem ein Bericht, als er „nichts da" sagte?
```

## L756 · `proved: bool,`

```
/// Hat er je RICHTIG einen Bericht angesagt? Steht einmal im Log.
```

## L761-767 · `const GATE_CROSS_CHECK_ROUNDS: u32 = 20;`

```
/// Gegenprobe: so viele uebersprungene Runden, dann wird trotzdem gelesen.
///
/// **Schweigen beweist nichts** — ein ruhendes Touchpad sagt nichts, und
/// ein Pin, der immer „nichts da" meldet, sieht genauso aus. Was etwas
/// beweist, ist der umgekehrte Fall: ein Bericht, der ankommt, OBWOHL der
/// Pin nein sagte. Alle 100 ms wird deshalb blind gelesen, und drei solche
/// Widersprueche hintereinander schalten das Tor dauerhaft ab.
```

## L769-770 · `const GATE_CROSS_CHECK_PROVED: u32 = 200;`

```
/// Hat der Pin einen Bericht einmal richtig ANGESAGT, taugt er — dann
/// reicht ein Herzschlag je Sekunde, und die Gegenprobe kostet nichts mehr.
```

## L774-779 · `fn talk_to_device(`

```
/// Mit dem GERAET reden: Bus einrichten, Adresse antippen, HID-Deskriptor
/// holen, aufwecken und zuruecksetzen.
///
/// Ab hier wird GESCHRIEBEN. Die Rechtfertigung ist der Registerwert, den
/// der Controller gerade selbst geliefert hat — nicht eine Firmware-Flagge,
/// die wir aus einem Namen errechnen, den wir nirgends finden.
```

## L816-818 · `let n = desc.report_desc_length as usize;`

```
// Den REPORT-DESKRIPTOR holen und AUSWERTEN. Was ein Byte im Bericht
// bedeutet, steht dort und nirgends sonst — ohne ihn gilt ein Treiber
// fuer genau ein Modell.
```

## L832-838 · `if n <= 512 {`

```
// Den Deskriptor ROH ins Log, wenn er klein genug ist.
//
// Mein Parser findet auf diesem Geraet EINEN Kontaktplatz, wo ein
// Praezisions-Touchpad fuenf deklariert. Das laesst sich nicht
// erraten — es steht in diesen Bytes, und sie sind die Grundwahrheit,
// nicht meine Auslegung davon. 381 Bytes sind 16 Zeilen; die 893 der
// Wacom bleiben draussen.
```

## L846-852 · `let mut switched: Option<u8> = None;`

```
// Wenn das Geraet einen „Device Mode" fuehrt, auf 3 stellen.
//
// Ein Praezisions-Touchpad startet in der MAUS-Nachahmung: ein X, ein
// Y, Tasten — und keine Kontaktpunkte. Zweifinger-Scrollen ist in
// diesem Zustand nicht schwer, sondern unmoeglich, weil der zweite
// Finger gar nicht gemeldet wird. Der Schalter steht in einem
// Feature-Bericht (Digitizer 0x52).
```

## L857-864 · `let n = map.report_bytes(report::Kind::Feature, im.report_id).max(1);`

```
// Die Laenge kommt aus dem DESKRIPTOR, nicht aus dem Bauch.
//
// Florians Elan fuehrt `Input Mode` mit `Report Size 16` — der
// Feature-Bericht ist ZWEI Bytes lang. Wir schickten eines. Auf
// dem Bus quittiert das Geraet, der Bericht ist aber zu kurz und
// wird verworfen: kein Fehler, keine Wirkung, und danach kommt
// ewig nur die Maus-Nachahmung. Fuellbits zaehlen mit, deshalb
// rechnet `report_bytes` und nicht die Summe der Felder.
```

## L882-883 · `let mut decoders: alloc::vec::Vec<Decoder> = alloc::vec::Vec::new();`

```
// JEDEN Bericht einrichten, den wir lesen koennen — Touchpad und
// Maus. Welcher kommt, entscheidet das Geraet, nicht wir.
```

## L905-907 · `let span = (ys[0].logical_max - ys[0].logical_min).max(1);`

```
// Ein Scrollschritt aus dem logischen Bereich: etwa ein
// Vierzigstel der Padhoehe je Raste. Geraeteunabhaengig, weil
// die Zahl aus dem Geraet selbst kommt.
```

## L910-913 · `tap_move = ((xs[0].logical_max - xs[0].logical_min).max(1) / 80).max(1);`

```
// Soweit darf ein Finger wandern und es bleibt ein Tippen:
// rund ein Achtzigstel der Padbreite, also etwa 1,3 mm —
// derselbe Wert, den libinput nimmt. Aus dem Geraet
// hergeleitet, nicht in Pixeln geraten.
```

## L915-920 · `pin_move = (tap_move * 2).max(1);`

```
// Und soweit darf er unter einer GEDRUECKTEN Taste wandern,
// bevor der Zeiger ihm wieder folgt: doppelt so weit, also
// rund 2,6 mm. Wer durchdrueckt, verformt die Fingerkuppe,
// und ihr wandernder Schwerpunkt ist keine Zeigerbewegung.
// Aus unserem eigenen Tippmass hergeleitet und nicht aus
// einer Millimeterzahl geraten.
```

## L922-924 · `hscroll_step = (((xs[0].logical_max - xs[0].logical_min).max(1)) / 40).max(1);`

```
// Die quere Raste kommt aus der BREITE, nicht aus der Hoehe:
// das Pad ist breiter als hoch, und ein Schritt aus der Hoehe
// liefe quer zu fein.
```

## L995-1005 · `enum Step {`

```
/// Einen Eingabebericht abholen und als Zeiger oder Geste einspeisen.
///
/// Der Unterschied, um den sich alles dreht: eine Maus meldet WEGE, ein
/// Touchpad ORTE. Aus Orten wird ein Weg, indem man den vorigen abzieht —
/// und die ERSTE Beruehrung liefert keinen, sonst spraenge der Zeiger
/// dorthin, wo der Finger aufsetzt.
///
/// Und eine GESTE meldet niemand. „Zwei Finger wandern parallel" steht in
/// keinem Bericht; es entsteht erst hier, aus der Zahl der aufliegenden
/// Kontaktpunkte und ihrer Bewegung. Unter Linux macht das libinput.
/// Was ein einzelner Leseversuch ergeben hat.
```

## L1007-1008 · `Data,`

```
/// Ein Bericht kam, und wir konnten ihn lesen — es kann sofort noch
/// einer dahinter liegen.
```

## L1010 · `Empty,`

```
/// Nichts da. Das Geraet lebt, hat aber gerade nichts zu sagen.
```

## L1012-1025 · `Junk,`

```
/// **Etwas kam, aber es ist kein Bericht.**
///
/// Eine Nummer, die der Deskriptor des Geraets SELBST nicht fuehrt.
/// Florians Wacom antwortet auf eine Lesung ohne anliegende Daten mit
/// ID 255 — seine eigenen sind 28, 19, 20, 11, 16, 31, 1 —, der Elan
/// mit ID 0, statt mit Laenge 0, wie HID over I2C es vorsieht. Genau
/// diese Frage stellt `docs/plan/INPUT_I2C_HID.md` seit je: was sagt
/// ein Geraet, wenn man es ohne Grund anspricht.
///
/// Das ist KEINE Information. Es darf deshalb weder die Drainschleife
/// weiterlaufen lassen (acht Uebertragungen je Runde, fuer nichts)
/// noch als Beweis GEGEN den Interrupt-Pin zaehlen — und genau das
/// hat es in 0.23/0.24 getan: drei solche Antworten haben ein
/// funktionierendes Tor abgeschaltet.
```

## L1027 · `Dead,`

```
/// Der Bus antwortet nicht mehr.
```

## L1031-1034 · `const MAX_GPIO_MAPS: usize = 2;`

```
// ── Das Tor am Interrupt-Pin ─────────────────────────────────────────
//
// Eine Abbildung je GPIO-Block, nicht je Geraet: beide Geraete dieses
// Notebooks haengen am selben, und `MAX_MMIO_MAPS` ist vier.
```

## L1040 · `let maps = unsafe { &mut *core::ptr::addr_of_mut!(GPIO_MAPS) };`

```
// SAFETY: ein Faden, ein Lauf — das Modul hat keine Nebenlaeufigkeit.
```

## L1054-1057 · `fn arm_gate(d: &i2c_hid_core::discover::HidDevice) -> Gate {`

```
/// Das Tor scharf machen — oder begruenden, warum nicht.
///
/// Jede Absage steht im Log. Ein Treiber, der still blind pollt, sieht
/// genauso aus wie einer, der es nicht tut.
```

## L1073-1075 · `if !gpio::is_amd_block(&g.ids) {`

```
// Der Registeraufbau ist AMD-eigen. Ein fremder Block an derselben
// Stelle fuehrt etwas anderes, und ein geratenes Bit 16 waere
// schlimmer als gar keine Abfrage.
```

## L1082-1083 · `if !d.gpio_level_triggered() {`

```
// Eine FLANKE laesst sich nicht abfragen: im Augenblick des Hinsehens
// ist sie vorbei. Nur ein Pegel steht an, bis der Bericht geholt ist.
```

## L1120 · `fn gate_asserted_now(l: &mut Live) -> bool {`

```
/// Liegt gerade etwas an? Ohne Tor lautet die Antwort immer ja.
```

## L1131 · `fn gate_check(l: &mut Live) -> (bool, bool) {`

```
/// Soll diese Runde gelesen werden — und sagte der Pin dabei nein?
```

## L1151-1155 · `fn gate_verdict(l: &mut Live, gate_said_no: bool, got_data: bool) {`

```
/// Was die Gegenprobe ergeben hat.
///
/// Abgeschaltet wird das Tor nur durch einen WIDERSPRUCH — ein Bericht,
/// der ankam, obwohl der Pin nichts meldete. Dass nichts kommt, beweist
/// gar nichts: ein unberuehrtes Touchpad schweigt.
```

## L1163-1169 · `if !*proved {`

```
// Eine RICHTIGE Ansage loescht die Widersprueche.
//
// Ein Widerspruch kann auch ein Wettlauf sein: der Finger
// setzt genau in der Gegenprobe auf, Mikrosekunden nachdem
// der Pin gelesen wurde. Das passiert einzeln. Ein FALSCHES
// Tor dagegen widerspricht bei jeder Gegenprobe und sagt
// nie etwas richtig an — nur DAS soll es abschalten.
```

## L1200-1203 · `if l.err_logged < 8 {`

```
// Der Grund wurde bisher WEGGEWORFEN. Ein Timeout und ein
// AddrNack sehen von aussen gleich aus und kosten das
// Tausendfache voneinander: der eine kehrt sofort zurueck,
// der andere haelt xfer bis zu einer Sekunde fest.
```

## L1214-1216 · `let Some(d) = l.decoders.iter().find(|d| d.rid == id) else {`

```
// Den Decoder zu DIESER Nummer nehmen. Kennt ihn keiner, einmal
// sagen, welche Nummer kam — das ist die Auskunft, die fehlt, wenn
// sich nichts bewegt.
```

## L1244-1248 · `if !btn.is_empty() {`

```
// Die physische Tastenlage merken — aber nur aus einem Bericht, der
// ueberhaupt Tasten FUEHRT. Ein Geraet, das seine Taste in einem
// eigenen Bericht meldet, setzte sie sonst mit dem naechsten
// Kontaktbericht still wieder zurueck, und das Festhalten unter dem
// Druck waere wirkungslos, ohne dass es irgendwo auffiele.
```

## L1262 · `let s = wheel.as_ref().map(|w| report::extract(data, w)).unwrap_or(0);`

```
// Das Rad meldet immer RELATIV — Rasten, keine Position.
```

## L1280 · `let cid = c.id.as_ref()`

```
// Ohne Kennungsfeld ist der PLATZ die Kennung.
```

## L1290-1293 · `match l.track.feed(cc, &present[..np], contacts.len(), now_ms, buttons != 0) {`

```
// Die Taste faehrt MIT: ein durchgedruecktes Pad haelt die
// Finger fest, und eine Beruehrung unter der Taste ist kein
// Antippen. Beides gehoert in den Tracker, weil es dort
// Tests hat.
```

## L1319-1324 · `push_buttons(l);`

```
// Erst die LAGE, dann der Impuls, dann der Weg.
//
// Die Reihenfolge ist nicht beliebig: beim zweiten Antippen einer
// Reihe gibt der Tracker im selben Bild „Haltetaste auf" UND „ein
// ganzer Klick". Kaeme der Klick zuerst, stuende er IN der noch
// gedrueckten Taste und der Compositor saehe nur einen.
```

## L1337-1342 · `fn push_buttons(l: &mut Live) {`

```
/// Die Tastenlage melden, wenn sie sich geaendert hat.
///
/// **Die einzige Stelle, die sie bildet.** Sie kommt aus zwei Quellen —
/// den physischen Tasten des letzten Berichts und der Taste, die ein
/// Antippen gerade haelt —, und zwei Stellen, die das je fuer sich
/// zusammenrechnen, waeren zwei Semantiken.
```

