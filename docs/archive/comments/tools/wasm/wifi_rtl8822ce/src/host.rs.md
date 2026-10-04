# `tools/wasm/wifi_rtl8822ce/src/host.rs` @ 5e0102684

## L1-6 · `#![allow(dead_code)]`

```
//! nopeekOS WASM Driver ABI — Bindings fuer den RTL8822CE-Treiber.
//!
//! Dieselbe ABI, die `wifi_ax200` und `wifi` (RTL8852BE) fahren, plus die
//! zwei 8-Bit-Zugriffe, die es fuer rtw88 geben MUSS: der
//! Power-Sequenz-Interpreter in `mac.c` besteht aus nichts anderem als
//! `rtw_read8`/`rtw_write8`, und ein 32-Bit-RMW ist dafuer kein Ersatz.
```

## L44-46 · `fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;`

```
// ── Stufe 6a: der Steuerkanal und der Datenweg ───────────────
// docs/spec/WIFI_CLASS_ABI.md §3. Die Treiberseite ist an den
// gebundenen Treiber gegated; `wifid` sitzt am anderen Ende.
```

## L55 · `fn npk_irq_register(entry: i32) -> i32;`

```
// ── Interrupt statt Abfrage (docs/plan/CORES_AND_EVENTS.md, Stufe 2c)
```

## L60 · `pub const WAIT_IRQ: i32 = 2;`

```
// ── Warten auf Ereignisse ────────────────────────────────────────
```

## L62 · `pub const WAIT_IRQ: i32 = 2;`

```
/// `npk_wait`-Bits (Kernel `host_core::npk_wait`).
```

## L67-68 · `pub fn irq_register() -> i32 {`

```
/// Den MSI des gebundenen Geraets anmelden. Der Vektor, oder `-1`, wenn
/// es keinen gibt — dann bleibt der Treiber im Abfragebetrieb.
```

## L73-74 · `pub fn wait(mask: i32, timeout_ms: u32) -> i32 {`

```
/// Parken, bis eines der Ereignisse in `mask` eintritt oder `timeout_ms`
/// vergeht. Die eingetretenen Bits, 0 bei Fristablauf.
```

## L79 · `pub fn fetch(name: &str, buf: &mut [u8]) -> i32 {`

```
// ── Stufe 6a: Steuerkanal, npkFS und Datenweg ────────────────────
```

## L81-82 · `pub fn fetch(name: &str, buf: &mut [u8]) -> i32 {`

```
/// Ein Objekt aus npkFS holen. Gibt die Laenge zurueck, `-1` wenn es
/// nicht da ist — und das ist ein gewoehnlicher Fall, kein Fehler.
```

## L90 · `pub fn wifi_poll_cmd(buf: &mut [u8]) -> i32 {`

```
/// Naechstes Kommando von `wifid`, `-1` = keins.
```

## L95 · `pub fn wifi_send_event(msg: &[u8]) -> i32 {`

```
/// Ereignis an `wifid`.
```

## L104 · `pub fn netdev_submit_rx(frame: &[u8]) -> i32 {`

```
/// Ein empfangenes Ethernet-Rahmen an den IP-Stapel.
```

## L109 · `pub fn netdev_poll_tx(buf: &mut [u8]) -> i32 {`

```
/// Ein zu sendendes Ethernet-Rahmen holen, `-1` = keins.
```

## L118-129 · `static VERBOSE: AtomicBool = AtomicBool::new(false);`

```
// ── Ausgabe: laut oder leise ─────────────────────────────────────
//
// **Im Autostart ist Stille die Vorgabe.** Der Treiber druckt sechs
// Stufen mit ihren Toren — das ist der Grund, warum sie entstanden sind,
// ohne im Dunkeln zu suchen, und es macht die Konsole fuer alles andere
// unbrauchbar. `debug: 1` in `sys/config/wifi` schaltet sie wieder an.
//
// **Still heisst nicht stumm.** Was nicht stimmt, geht immer hinaus:
// `say` fuer eine Zeichenkette, `loud_begin`/`loud_end` als Klammer um
// eine zusammengesetzte Zeile. Die Klammer ist der Grund, warum es keine
// zweite Garnitur Zahlenformatierer braucht — `print_dec` und die
// anderen rufen `print`, und das sieht die Klammer.
```

## L141-142 · `pub fn loud_begin() {`

```
/// Alles bis `loud_end` geht auch ohne `debug: 1` hinaus. Gezaehlt und
/// nicht geschaltet, damit ein Rufer den anderen nicht abstellt.
```

## L148-149 · `let v = LOUD.load(Ordering::Relaxed);`

```
// Saettigend: eine Klammer, die ohne Anfang schliesst, darf nicht
// unter null laufen und damit jede Ausgabe anschalten.
```

## L158 · `pub fn print(s: &str) {`

```
/// Die Stufenausgabe — nur mit `debug: 1` oder innerhalb von `loud_*`.
```

## L165 · `pub fn say(s: &str) {`

```
/// Was immer hinausgeht: Fehler, gefallene Tore, der Zustand der Leitung.
```

## L174 · `pub fn pci_bind(vendor: u16, device: u16) -> i32 {`

```
// ── PCI ──────────────────────────────────────────────────────────
```

## L192 · `pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {`

```
// ── MMIO ─────────────────────────────────────────────────────────
```

## L198 · `pub fn r8(h: i32, off: u32) -> u8 {`

```
/// `rtw_read8`. Echter 8-Bit-Buszugriff, kein RMW.
```

## L203-204 · `pub fn w8(h: i32, off: u32, val: u8) {`

```
/// `rtw_write8`. Echter 8-Bit-Buszugriff — die drei Nachbarbytes bleiben
/// unberuehrt, und genau das ist der Unterschied zu einem 32-Bit-RMW.
```

## L225 · `pub fn set8(h: i32, off: u32, bits: u8) {`

```
/// `rtw_write8_set` — Bits im BYTE setzen, gelesen und geschrieben in 8 Bit.
```

## L230 · `pub fn clr8(h: i32, off: u32, bits: u8) {`

```
/// `rtw_write8_clr`
```

## L235 · `pub fn set32(h: i32, off: u32, bits: u32) {`

```
/// `rtw_write32_set`
```

## L240 · `pub fn clr32(h: i32, off: u32, bits: u32) {`

```
/// `rtw_write32_clr`
```

## L245 · `pub fn dma_alloc(pages: u16) -> i32 {`

```
// ── DMA ──────────────────────────────────────────────────────────
```

## L247-248 · `pub fn dma_alloc(pages: u16) -> i32 {`

```
/// Zusammenhaengende Seiten unter 4 GB (der Kernel garantiert beides; der
/// TX-/RX-Deskriptor hat nur ein 32-Bit-Adressfeld).
```

## L253-254 · `pub fn dma_alloc_below(pages: u16, limit_mb: u32) -> i32 {`

```
/// Wie `dma_alloc`, aber unter einer selbst genannten Obergrenze.
/// `limit_mb = 0` heisst 4 GB, also dasselbe wie `dma_alloc`.
```

## L264-265 · `pub fn dma_write_buf(handle: i32, offset: u32, data: &[u8]) -> i32 {`

```
/// Bytes aus dem Linearspeicher in den DMA-Puffer. 4096 Bytes ueber 1024
/// Einzelschreibungen waeren dieselbe Wirkung zum vielfachen Preis.
```

## L282 · `pub fn fence() {`

```
// ── Zeit und Bericht ─────────────────────────────────────────────
```

## L306 · `const HEX: &[u8; 16] = b"0123456789abcdef";`

```
// ── Hex/Dez ohne alloc ───────────────────────────────────────────
```

## L346-347 · `pub fn log_reg32(name: &str, val: u32) {`

```
/// Ein Register mit Namen und Wert — die Zeile, aus der spaeter jede
/// Diagnose kommt.
```

## L356-357 · `pub fn w32_mask(h: i32, off: u32, mask: u32, data: u32) {`

```
/// hci.h:241-253 `rtw_write32_mask` — Feld an seiner Schiebestelle setzen.
/// `data` ist der Wert des FELDES, nicht das fertige Bitmuster.
```

## L364 · `pub fn r32_mask(h: i32, off: u32, mask: u32) -> u32 {`

```
/// hci.h:202-212 `rtw_read32_mask`
```

## L369-373 · `pub fn delay_us(us: u64) {`

```
/// `udelay(n)`. Unter einer Millisekunde kann `npk_sleep` nichts, also wird
/// auf der Uhr gedreht. Der Preis ist ehrlich: ein `npk_now_us` kostet selbst
/// etwa so viel wie die kuerzeste Pause, die hier verlangt wird (1 us nach
/// jedem RF-Schreibzugriff), und laenger zu warten als noetig ist harmlos —
/// kuerzer waere es nicht.
```

## L379 · `pub fn set16(h: i32, off: u32, bits: u16) {`

```
/// `rtw_write16_set`
```

## L384 · `pub fn clr16(h: i32, off: u32, bits: u16) {`

```
/// `rtw_write16_clr`
```

## L389-390 · `pub fn w8_mask(h: i32, off: u32, mask: u32, data: u8) {`

```
/// hci.h:255-266 `rtw_write8_mask` — Feld im BYTE setzen.
/// `mask` wird vorher auf acht Bit beschnitten, wie in Linux.
```

## L398-399 · `pub fn r16_mask(h: i32, off: u32, mask: u16) -> u16 {`

```
/// `rtw_write16_set` fuer ein Feld — es gibt in Linux kein
/// `rtw_write16_mask`, aber `rtw_write16_set` mit einer Maske.
```

