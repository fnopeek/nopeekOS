# `tools/wasm/audio_hda/src/lib.rs` @ 5e0102684

## L1-13 · `#![no_std]`

```
//! audio_hda — generic Intel HD Audio (HDA) controller driver (WASM module).
//!
//! Hardware-independent by construction: binds the HDA controller by PCI *class*
//! (0x04/0x03), not vendor:device, so it drives any HDA-spec controller (Intel,
//! AMD, NVIDIA, QEMU's intel-hda). The codec is enumerated generically by walking
//! the widget graph (like `snd-hda-codec-generic`) to find a DAC -> output-pin path.
//!
//! Codec verbs use the spec's Immediate Command Interface (IC/IR/IRS) — what Linux
//! uses as `single_cmd` on Intel — so the only DMA is the audio ring + BDL.
//!
//! M1: bring the controller + codec + one output stream up and play a built-in
//! sine tone. Proves the whole path before the kernel audio mailbox (M2) exists.
//! Test target: bare metal (audible yes/no). Verbose stage banners over serial.
```

## L28-29 · `#[unsafe(link_section = ".npk.caps")]`

```
// Driver only needs to bind a PCI device -> EXECUTE right. The default caps
// grant READ|EXECUTE|RENDER; we declare EXECUTE explicitly (least privilege).
```

## L34-38 · `const HALF_FRAMES: usize = 2048; // ~43 ms per half @ 48 kHz`

```
// ── audio buffer geometry ───────────────────────────────────────────────
// 480 Hz tone @ 48 kHz = exactly 100 samples/period -> a clean cyclic loop.
// HDA playback ring: two ping-pong halves fed from the kernel audio mailbox.
// The DMA cycles the ring; each tick we keep the half it is NOT currently
// playing filled with freshly mixed PCM (silence when no app is playing).
```

## L39 · `const HALF_FRAMES: usize = 2048; // ~43 ms per half @ 48 kHz`

```
// ~43 ms per half @ 48 kHz
```

## L40 · `const HALF_BYTES: usize = HALF_FRAMES * 4; // S16 stereo`

```
// S16 stereo
```

## L45 · `static mut MIXBUF: [u8; HALF_BYTES] = [0; HALF_BYTES];`

```
// Scratch buffer for one mailbox poll -> one ring half.
```

## L48 · `fn loghex(prefix: &str, v: u32) {`

```
// ── small hex/dec logging helpers (no alloc) ──────────────────────────────
```

## L59-63 · `fn dbghexln(prefix: &str, v: u32) {`

```
/// Eine GANZE Diagnosezeile (Praefix, Hexwert, Zeilenende), nur mit
/// `set log.drivers 1`.
///
/// Bewusst inklusive Zeilenende: ein gegateter Anfang mit ungegatetem
/// `log("\n")` dahinter haette Leerzeilen gedruckt.
```

## L68 · `fn codec_cmd(mmio: i32, cad: u32, nid: u32, verb20: u32) -> Option<u32> {`

```
// ── Immediate-command codec access ────────────────────────────────────────
```

## L71 · `let mut spin = 0;`

```
// Wait until not busy.
```

## L77 · `mmio_w16(mmio, IRS, IRS_IRV); // clear stale result (W1C)`

```
// clear stale result (W1C)
```

## L80 · `mmio_w16(mmio, IRS, IRS_ICB); // send`

```
// send
```

## L86 · `mmio_w16(mmio, IRS, IRS_IRV); // clear`

```
// clear
```

## L102-130 · `fn unmute_out(mmio: i32, cad: u32, nid: u32) {`

```
/// Unmute the output amp of a widget at (near) max gain — **if it has one.**
///
/// Ein Verstaerker-Verb an ein Widget OHNE Verstaerker ist nach Spezifikation
/// undefiniert, und die beiden Pruefungen hier sind nicht Vorsicht, sondern
/// der Unterschied zwischen Ton und Stille:
///
/// QEMUs Line-Out-Pin meldet `AMP_OUT_CAP = 0` und traegt in seiner
/// Knotenbeschreibung kein `stindex` — der faellt damit auf 0 zurueck, also
/// auf DENSELBEN Strom wie der DAC. Ohne die Pruefung rechnet der Code
/// `steps = 0` -> `gain = 0`, schreibt das Verb trotzdem, und QEMU setzt
/// damit die Verstaerkung des DAC-Stroms auf null: `left = 0 * 255 / 74`.
/// Der Treiber stellt den DAC also korrekt ein und loescht ihn eine Zeile
/// spaeter selbst.
///
/// Der Satz „auf echter Hardware hat der Pin Stufen > 0" stimmte nicht.
/// Auf dem Realtek eines Lenovo IdeaPad meldet der LAUTSPRECHERpin
/// `steps = 0` — und ist trotzdem stumm, weil null Stufen NICHT „kein
/// Verstaerker" heisst, sondern „reiner Stummschalter". Der hat ein
/// Mute-Bit, und das blieb unangetastet: Strom lief, Pin stimmte, kein Ton.
///
/// Die Spezifikation trennt die beiden Faelle in Bit31 von `AMP_*_CAP`
/// („kann stummschalten"):
///
/// * Stufen 0 **und** Bit31 gesetzt → Stummschalter, MUSS aufgemacht werden
/// * Stufen 0 **und** Bit31 frei    → kann nichts, Verb schadet nur (QEMU,
///   dessen Pin `AMP_OUT_CAP = 0` meldet — Bit31 also ebenfalls frei)
///
/// Damit bleibt der QEMU-Fall oben Wort fuer Wort gueltig, und der Pin, der
/// nur stummschalten kann, geht trotzdem auf.
```

## L132-133 · `if get_param(mmio, cad, nid, PARAM_AUDIO_WIDGET_CAP) & WCAP_OUT_AMP == 0 {`

```
// Beides fragen, weil beides unabhaengig „nein" sagen kann: das
// Faehigkeitsbit des Widgets und die Stufenzahl seines Verstaerkers.
```

## L141 · `codec_cmd(mmio, cad, nid, vset_amp(AMP_SET_OUT_BOTH));`

```
// Verstaerkung 0, Mute-Bit (Bit7) frei: aufgemacht.
```

## L145 · `let gain = ((steps * 3 / 4) & 0x7F) as u16;`

```
// ~3/4 of max gain — audible but not blasting (real volume control = M4).
```

## L150-156 · `fn unmute_in(mmio: i32, cad: u32, nid: u32, index: u32) {`

```
/// Entmutet den EINGANGSverstaerker `index` eines Knotens.
///
/// Ein Mixer hat je Eingang einen eigenen Verstaerker, und die stehen auf
/// vielen Codecs ab Werk auf stumm. `unmute_out` oeffnet nur den Ausgang —
/// damit ist Mixer → Pin frei und DAC → Mixer weiter zu. Das sieht aus wie
/// ein laufender Strom ohne Ton: LPIB laeuft, der Pin stimmt, es kommt
/// nichts.
```

## L165-166 · `if cap & AMP_CAP_MUTE == 0 { return; }`

```
// Dieselbe Trennung wie beim Ausgang: Stummschalter aufmachen,
// einen Verstaerker ohne jede Faehigkeit in Ruhe lassen.
```

## L175-181 · `fn amp_cap(mmio: i32, cad: u32, nid: u32) -> u32 {`

```
/// Die ROHE Verstaerkerfaehigkeit eines Knotens — fuer den Log.
///
/// Die Stufenzahl allein reicht nicht: `steps = 0` kann „kein Verstaerker",
/// „reiner Stummschalter" oder „Widget hat gar keinen Ausgangsverstaerker"
/// heissen, und die drei verlangen Verschiedenes. Bit31 = kann
/// stummschalten, [14:8] = Stufen, [6:0] = Offset. `0` heisst hier: das
/// Widget fuehrt gar keinen Ausgangsverstaerker.
```

## L188 · `let r = codec_cmd(mmio, cad, nid, vget_conn_entry(0)).unwrap_or(0);`

```
// Short-form connection list: 4 entries packed in one response.
```

## L193-194 · `fn trace_to_dac(mmio: i32, cad: u32, pin: u32) -> u32 {`

```
// Walk from an output pin back to its feeding DAC (up to 2 hops through a
// mixer/selector), unmuting each node in the path.
```

## L196-198 · `if host::verbose() {`

```
// Der Weg wird GEMELDET, nicht vermutet. Ohne ihn steht im Log nur
// "pin 0x14" und "DAC 0x02", und ob ein Mixer dazwischenliegt — also
// ob ueberhaupt ein Eingangsverstaerker im Spiel ist — bleibt offen.
```

## L214 · `if widget_type(mmio, cad, first) == WTYPE_SELECTOR {`

```
// mixer or selector: select input 0, unmute BOTH directions, descend
```

## L235-241 · `fn setup_codec(mmio: i32, cad: u32) -> (u32, bool) {`

```
// Generic codec setup: find an output pin + its DAC, configure format/stream,
// enable the pin, unmute the path. Returns the DAC NID or 0 on failure.
/// Richtet den Codec ein. Zweiter Rueckgabewert: **ist der gewaehlte
/// Ausgang ANALOG?** (Lautsprecher, Kopfhoerer oder Line-Out). Daran
/// entscheidet `_start`, ob dieser Controller der richtige ist — eine
/// HDMI-Audioeinheit hat nur `dev=0x05`, und ein Ton dorthin ist auf
/// Lautsprechern nicht zu hoeren.
```

## L243 · `let root = get_param(mmio, cad, 0, PARAM_SUB_NODE_COUNT);`

```
// Function groups under the root node.
```

## L259 · `codec_cmd(mmio, cad, afg, vset_power(0)); // D0`

```
// D0
```

## L261 · `let w = get_param(mmio, cad, afg, PARAM_SUB_NODE_COUNT);`

```
// Widgets under the AFG.
```

## L266-268 · `let mut pin = 0u32;`

```
// Find the best output pin. Prefer the built-in speaker (so a tone is
// audible with no cable), then headphone, then line-out. Log every
// output-capable pin so the real codec topology is visible on hardware.
```

## L276 · `if pcap & (1 << 4) == 0 { continue; } // not output-capable`

```
// not output-capable
```

## L279 · `let conn = (cfg >> 30) & 0x3; // 1 = no physical connection`

```
// 1 = no physical connection
```

## L280 · `let dev = (cfg >> 20) & 0xF; // 0=LineOut 1=Speaker 2=HPOut`

```
// 0=LineOut 1=Speaker 2=HPOut
```

## L287 · `if conn == 1 { continue; } // no physical jack/connection`

```
// no physical jack/connection
```

## L318 · `codec_cmd(mmio, cad, dac, vset_power(0));`

```
// Configure the DAC: power, format, bind to our stream tag, unmute.
```

## L324 · `codec_cmd(mmio, cad, pin, vset_power(0));`

```
// Enable the pin: power, output enable, EAPD, unmute.
```

## L333 · `const MAX_HDA: u32 = 4;`

```
// ── controller bring-up ───────────────────────────────────────────────────
```

## L335 · `const MAX_HDA: u32 = 4;`

```
/// Hoechste Zahl an HD-Audio-Controllern, die durchprobiert wird.
```

## L338-340 · `fn bring_up() -> Option<(i32, u32, bool)> {`

```
/// Die Bindung steht bereits: Controller hochfahren und den Codec
/// einrichten. Gibt `(mmio, iss, analog)` oder `None`, wenn dieser
/// Controller nichts taugt — der Rufer geht dann zum naechsten.
```

## L343 · `let tcsel = pci_read_config(0x44);`

```
// Intel quirk: clear TCSEL (PCI 0x44) traffic-class bits so DMA uses TC0.
```

## L352 · `let iss = (gcap >> 8) & 0xF; // input streams (output streams follow them)`

```
// input streams (output streams follow them)
```

## L370 · `let g = mmio_r32(mmio, GCTL);`

```
// Assert reset (CRST=0), wait, then deassert (CRST=1), wait for run.
```

## L384 · `sleep_ms(1);`

```
// Codecs need time to report presence after reset.
```

## L409-425 · `let mut chosen: Option<(i32, u32)> = None;   // (mmio, iss)`

```
// Bind the HDA controller by PCI class — hardware-independent, no
// vendor:device hardcode. Intel cAVS controllers report subclass 0x01
// ("Audio controller") instead of the canonical 0x03 ("HD Audio"); both
// expose the same HDA register interface, so accept either.
// Den richtigen Controller SUCHEN, nicht den ersten nehmen.
//
// Fast jede Maschine hat zwei HD-Audio-Controller: den der GPU (HDMI/DP)
// und den der Southbridge (Lautsprecher, Kopfhoerer). Welcher in der
// PCI-Reihenfolge zuerst steht, ist Zufall — auf einem Lenovo IdeaPad
// stand die HDMI-Einheit vorn, und der Ton lief korrekt erzeugt in einen
// DisplayPort, an dem nichts haengt. Sechs Ausgangspins, alle dev=0x05
// ("Digital Other Out"), kein einziger analoger.
//
// Das Urteil gehoert hierher und nicht in den Kernel: der reicht den
// n-ten Controller der Klasse heraus, was ein brauchbarer Ausgang ist,
// weiss nur dieser Treiber. Intel cAVS meldet Unterklasse 0x01 statt
// 0x03 — beide Listen werden durchgegangen.
```

## L426 · `let mut chosen: Option<(i32, u32)> = None;   // (mmio, iss)`

```
// (mmio, iss)
```

## L427 · `let mut fb: Option<(u8, u32)> = None;        // erster brauchbarer, aber digital`

```
// erster brauchbarer, aber digital
```

## L439-441 · `if let Some((sub, i)) = fb {`

```
// Keiner hat einen analogen Ausgang — dann der erste, der ueberhaupt
// lief. Eine Maschine, die nur ueber HDMI ausgibt, soll nicht
// schlechter dastehen als vorher.
```

## L454 · `let audio = dma_alloc(((RING_BYTES + 4095) / 4096) as u16);`

```
// ── DMA: the playback ring (zeroed by the kernel = silence) + its BDL ──
```

## L463 · `let half = HALF_BYTES as u32;`

```
// BDL: two equal cyclic halves (HDA wants >= 2; LVI = entries-1).
```

## L469 · `buf[i + 12..i + 16].copy_from_slice(&1u32.to_le_bytes()); // IOC: one interrupt per half`

```
// IOC: one interrupt per half
```

## L476 · `let base = SD_BASE + iss * SD_STRIDE; // first output stream`

```
// ── program + start the output stream descriptor ──────────────────────
```

## L477 · `let base = SD_BASE + iss * SD_STRIDE; // first output stream`

```
// first output stream
```

## L485-487 · `let irq = host::irq_register() >= 0;`

```
// The controller's MSI (`azx_acquire_irq`; the AMD SB preset allows MSI).
// With it the stream raises an interrupt at the end of each half
// (IOC in both BDL entries) and the loop below sleeps until then.
```

## L491-494 · `let ic = mmio_r32(mmio, INTCTL);`

```
// `snd_hdac_stream_start`: the stream's bit in INTCTL plus the
// global enable, and `SD_INT_MASK` in SD_CTL with the run bit. The
// controller-interrupt enable (CIE) stays off: codec verbs are
// polled, only during bring-up, so no RIRB interrupt is wanted.
```

## L508-520 · `let mut write_pos: usize = 0;`

```
// Streaming loop: a TRUE ring-buffer copy. Each poll we refill exactly the
// region the DMA has played since last time — [write_pos, LPIB) — pulling
// that many bytes from the kernel mixer. This locks the drain rate to the
// DMA's real 48 kHz REGARDLESS of how often this loop is scheduled.
//
// The old ping-pong refilled one whole half only when it *detected* a buffer
// crossing, so coarse/jittery wakeups (npk_sleep is bounded by the worker
// timer → ~10-30 ms effective under load) missed crossings → the mailbox
// drained at only ~74 % of 48 kHz → the whole pipeline (which back-pressures
// on mailbox room) ran slow + delayed (HW-confirmed: mailbox pinned full,
// completion throttled to 74 %). write_pos chases LPIB; the ring stays ~one
// lap ahead of the play head (~85 ms latency, reported to the guest as the
// HDA-ring latency). poll_mix yields silence when no app is playing.
```

## L522-531 · `let mut reports = 5u32;`

```
// Erste fuenf Sekunden: sagen, ob die DMA ueberhaupt laeuft.
//
// Der Loop unten fuellt nur, was die DMA SCHON GESPIELT hat
// (`avail = LPIB - write_pos`). Steht LPIB still, wird nie etwas
// geschrieben — und das sieht im Log aus wie ein sauberer Start ohne
// Ton. Genau dieser Fall trat unter QEMU auf, und er war von „Senke
// stumm" nicht zu unterscheiden, weil niemand LPIB gemeldet hat.
//
// Selbstbegrenzt: nach fuenf Berichten still, damit ein Treiber, der
// immer laeuft, das Log nicht flutet.
```

## L535-537 · `let mut loud_reports = 6u32;`

```
// Berichte, die NUR feuern, wenn wirklich Ton durchlief. Die fuenf
// Sekundenberichte oben treffen die Stille am Anfang; ein Beep kommt
// spaeter und waere sonst nie im Log.
```

## L549-550 · `loghex(" SDCTL=0x", mmio_r32(mmio, base + SD_CTL));`

```
// SD_CTL als u32 gelesen traegt STS im obersten Byte (Offset
// 0x03) — Lauf-Bit, Stream-Tag und Status in einer Zahl.
```

## L565 · `let avail = ((lpib + RING_BYTES - write_pos) % RING_BYTES) & !3;`

```
// Bytes the DMA has played since we last filled (frame-aligned to 4).
```

## L568 · `let n = avail.min(RING_BYTES - write_pos).min(HALF_BYTES);`

```
// Don't cross the ring end or overflow MIXBUF in one copy.
```

## L572-574 · `let mut i = 0usize;`

```
// Lautstaerke des gemischten Blocks: sagt, ob aus der Mailbox
// ueberhaupt etwas kommt. Nur jedes achte Sample, das reicht fuer
// eine Spitze und kostet ein Achtel.
```

## L583-585 · `if n >= 4 {`

```
// Und zurueckholen, was eben geschrieben wurde. Stimmt das nicht
// ueberein, landet `dma_write` nicht dort, wo das Geraet liest —
// und dann ist jede Zeile darueber eine Behauptung.
```

## L594-602 · `if mmio_r32(mmio, INTSTS) & (1 << iss) != 0 {`

```
// `azx_interrupt` → `snd_hdac_bus_handle_stream_irq`: the
// stream's bit in INTSTS, then SD_STS cleared with SD_INT_MASK
// (write-1-to-clear) — as a BYTE, like Linux' `writeb`.
//
// 0.4.0 wrote it as the top byte of a 32-bit write to SD_CTL.
// QEMU's intel-hda models SD_CTL (3 bytes) and SD_STS (1 byte)
// as separate registers, so the status never cleared and every
// register update sent another MSI: 21 330 wakes a second, the
// sound still playing.
```

## L606-609 · `wait_irq(HALF_MS + 10);`

```
// **Sleep until the DMA finishes a half.** The poll was every
// 4 ms — 250 wakes a second, silence included. One half is
// HALF_FRAMES at 48 kHz; if the interrupt never came we would
// still refill a half-period late, with the other half queued.
```

## L617 · `const HALF_MS: u32 = (HALF_FRAMES as u32 * 1000) / 48_000;`

```
/// Play time of one ring half.
```

