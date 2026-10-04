# `tools/wasm/audio_hda/src/host.rs` @ 5e0102684

## L1-2 · `#[link(wasm_import_module = "env")]`

```
//! Host-function bindings for the nopeekOS WASM Driver ABI.
//! Subset needed by the HDA driver: serial log, PCI, MMIO, DMA, sleep, fence.
```

## L4-6 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L12 · `fn npk_pci_bind_class(class: i32, subclass: i32) -> i32;`

```
// PCI
```

## L19 · `fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;`

```
// MMIO (handle-based; no pointer deref)
```

## L27 · `fn npk_dma_alloc(pages: i32) -> i32;`

```
// DMA
```

## L38 · `fn npk_audio_poll_mix(ptr: i32, max: i32) -> i32;`

```
// Audio mailbox (driver side): pull a mixed S16/48k/stereo buffer.
```

## L51-56 · `static mut VERBOSE: bool = false;`

```
// ── Diagnosezeilen: gebaut, aber im Normalbetrieb still ──────────────
//
// Die Codec-Topologie und die Sekundenberichte (LPIB/wpos/SDCTL) sind das
// Werkzeug, mit dem dieser Treiber gebaut wurde — sie sagen, ob die DMA
// ueberhaupt laeuft. Im Normalbetrieb sagt das niemandem etwas.
// EINMAL beim Start gefragt; `set log.drivers 1` holt sie zurueck.
```

## L59 · `pub fn log_init() {`

```
/// Einmal beim Start: will der Nutzer den Mitschrieb sehen?
```

## L61 · `unsafe {`

```
// SAFETY: ein Faden, ein Lauf — einmal gesetzt, danach nur gelesen.
```

## L68 · `unsafe { core::ptr::addr_of!(VERBOSE).read() }`

```
// SAFETY: siehe `log_init`.
```

## L76-78 · `pub fn pci_bind_class_n(class: u8, subclass: u8, index: u32) -> i32 {`

```
/// Den `index`-ten Controller dieser Klasse binden. Eine Maschine hat fast
/// immer ZWEI HD-Audio-Controller (GPU-HDMI und Southbridge), und welcher
/// zuerst kommt, ist Zufall der PCI-Reihenfolge.
```

## L128 · `pub const WAIT_IRQ: i32 = 2;`

```
/// `npk_wait`-Bit: der IRQ des gebundenen Geraets hat gefeuert.
```

## L131 · `pub fn irq_register() -> i32 {`

```
/// Den MSI des gebundenen Controllers anmelden; der Vektor oder -1.
```

## L136 · `pub fn wait_irq(timeout_ms: u32) -> i32 {`

```
/// Parken, bis der IRQ feuert oder `timeout_ms` vergeht.
```

## L141-145 · `pub fn dma_read32(h: i32, off: u32) -> u32 {`

```
/// Ein Doppelwort aus dem DMA-Puffer zurueckholen.
///
/// Diagnose: ob `dma_write` wirklich in dem Speicher landet, den das Geraet
/// liest. Ohne Rueckweg ist „geschrieben" eine Behauptung — und unter QEMU
/// sah genau das aus wie ein gesunder Stream ohne Ton.
```

