//! Woher `crypto.getRandomValues` seine Bytes nimmt.
//!
//! **Die Engine hat keine Hostfunktionen — der Wirt leiht ihr eine**, genau
//! wie bei der Uhr (`Engine::set_clock`). Hier ist es der CSPRNG des Kernels
//! (ChaCha20, aus RDRAND geseedet, alle 64 Bloecke neu verschluesselt), den
//! beak ueber `npk_random_bytes` hereinreicht.
//!
//! **Und wenn keine Quelle da ist, gibt es keinen Zufall.** `Math.random`
//! einzusetzen waere die naheliegende Bequemlichkeit und der schlimmere
//! Fehler: Seitencode baut aus `getRandomValues` Sitzungsmarken, und eine
//! vorhersagbare Marke ist schlechter als eine fehlende Funktion — die
//! fehlende sieht man.

/// Die Quelle, die der Wirt eingereicht hat. `None` heisst: es gibt keine,
/// und `crypto` erscheint dann gar nicht erst.
static mut SOURCE: Option<fn(&mut [u8]) -> bool> = None;

/// Den Zufall des Wirts einreichen. Einmal beim Start; die Engine ruft ihn
/// danach selbst.
pub fn set_source(f: fn(&mut [u8]) -> bool) {
    unsafe { core::ptr::addr_of_mut!(SOURCE).write(Some(f)) };
}

/// Gibt es eine Quelle? Danach entscheidet sich, ob `crypto` im globalen
/// Objekt steht — eine Seite prueft `if (window.crypto)`, und die Antwort
/// muss der Wahrheit entsprechen.
pub fn available() -> bool {
    unsafe { core::ptr::addr_of!(SOURCE).read().is_some() }
}

/// Den Puffer fuellen. `false`, wenn keine Quelle da ist oder der Wirt
/// abgelehnt hat — der Rufer wirft dann, statt schwachen Zufall zu liefern.
pub fn fill(out: &mut [u8]) -> bool {
    let Some(f) = (unsafe { core::ptr::addr_of!(SOURCE).read() }) else { return false };
    f(out)
}
