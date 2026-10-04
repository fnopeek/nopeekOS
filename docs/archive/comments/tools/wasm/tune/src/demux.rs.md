# `tools/wasm/tune/src/demux.rs` @ 5e0102684

## L1-10 · `use alloc::boxed::Box;`

```
//! Der Containerschnitt: EINE Datei, zwei Stroeme.
//!
//! Bis hierher kannte tune nur [`Source`](crate::source::Source) — einen
//! Strom von Tonbloecken. Ein Film hat zwei, und sie muessen aus DERSELBEN
//! Lesung des Containers kommen: zwei Parser auf einer Datei sind zwei
//! Gelegenheiten, verschiedener Meinung zu sein.
//!
//! **Der Weg fuer Ton allein wird dadurch nicht laenger.** Eine MP3 ist ein
//! Demux mit einem Strom; `open` gibt sie unveraendert an `source::open`
//! weiter, und kein Byte geht durch neuen Code.
```

## L22-26 · `pub audio_priming: u64,`

```
/// Vorlaufsamples der TONspur, in deren eigener Rate — was vor dem
/// ersten hoerbaren Sample liegt. Steht hier und nicht hinter dem
/// `Source`-Vertrag, weil es eine Eigenschaft des CONTAINERS ist und
/// nicht des Dekoders: dieselbe Datei hat auf Video und Ton
/// verschiedene Werte (gemessen: Video 0, Ton 2112).
```

## L28-30 · `pub video_error: Option<OpenError>,`

```
/// Warum kein Bild da ist, wenn die Datei eigentlich eines haette.
/// `None` heisst „war nie eine Frage" — eine MP3 hat kein Bild und
/// schuldet dafuer keine Erklaerung.
```

## L34-35 · `pub fn open(bytes: &'static [u8]) -> Demux {`

```
/// Nach INHALT entscheiden, nicht nach Endung — dieselbe Regel, nach der
/// `source::open` seit je den Tondekoder waehlt.
```

## L45-47 · `return Demux { audio: None, video: None, audio_priming: 0, video_error: Some(OpenError::Fragmented) };`

```
// Die Sampletabellen stehen in den Fragmenten, nicht im `moov`. Das
// zu sagen ist eine andere Auskunft als „geht nicht", und nur eine
// davon ist unser Fehler.
```

