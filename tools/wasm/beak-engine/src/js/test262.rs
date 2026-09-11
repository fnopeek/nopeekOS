//! `$262` — das Wirtsobjekt des Konformanzlaeufers, und NUR dort.
//!
//! **Kein Sprachmerkmal.** test262 verlangt von jedem Wirt ein Objekt namens
//! `$262` mit einer Handvoll Haken, die eine Sprache selbst nicht anbieten
//! kann: einen Puffer abtrennen, ein Skript im globalen Bereich laufen
//! lassen, den Sammler rufen. Ohne es scheitern 455 Dateien (882 Varianten)
//! mit `ReferenceError` — an einer Luecke im GERUEST, nicht im Motor.
//!
//! **Und es erscheint nur, wenn der Wirt es bestellt hat** — dieselbe Regel
//! wie bei `crypto` (`super::random`): eine Seite darf `$262` nie sehen,
//! sonst hat sie einen Weg, Skripte am Zustellweg vorbei laufen zu lassen und
//! fremde Puffer unter den Sichten wegzuziehen.
//!
//! Was NICHT da ist, und warum es benannt statt still fehlt:
//!
//! * **`createRealm`** (200 Dateien) — ein zweiter Realm ist ein zweiter
//!   `Interp`; `Realm::drop` bricht dafuer eigens die Rc-Ringe
//!   ([[feedback_an_rc_ring_never_reaches_zero]]), und 973 KB je Realm sind
//!   gemessen. Ein eigener Posten, kein Nebenbei.
//! * **`IsHTMLDDA`** (34 Dateien) — der `[[IsHTMLDDA]]`-Exot (`document.all`):
//!   ein Objekt, das sich wie `undefined` VERHAELT. Das ist eine Aenderung am
//!   Wahrheitswert und am `typeof`, also am Objektmodell.
//! * **`agent`** — Atomics/SharedArrayBuffer, und die stehen in
//!   `SKIP_FEATURES_EXEC`. Ein Haken fuer etwas, das gar nicht laeuft, waere
//!   eine Zusage, die nicht haelt.

/// Hat der Wirt `$262` bestellt? Vorgabe: nein.
static mut ON: bool = false;

/// Vom Laeufer EINMAL beim Start gerufen. Die Engine fragt danach selbst.
pub fn enable() {
    unsafe { core::ptr::addr_of_mut!(ON).write(true) };
}

/// Steht `$262` im globalen Objekt?
pub fn enabled() -> bool {
    unsafe { core::ptr::addr_of!(ON).read() }
}
