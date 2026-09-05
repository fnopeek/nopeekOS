//! Geraete im eigenen Netz: angeheftetes Vertrauen statt „ignorieren".
//!
//! # Das Problem, und warum es keins von uns ist
//!
//! Ein Router unter `https://192.168.178.1` KANN kein oeffentlich vertrautes
//! Zertifikat haben. Keine CA stellt eines fuer eine private Adresse aus,
//! und der Name, auf den das Geraet sein selbstsigniertes ausstellt, ist
//! nicht die IP, die man tippt. Beides zusammen ergibt zwangslaeufig
//! `hostname mismatch` + `untrusted root`. Das ist eine Luecke in der
//! Web-PKI, kein Fehler in beak.
//!
//! # Warum nicht einfach ignorieren
//!
//! Weil das LAN kein sicherer Ort ist. Ein uebernommenes Geraet im selben
//! Netz — eine Kamera, ein Drucker, irgendein Ding — koennte den Router
//! dann unbemerkt spielen, und der Nutzer gibt dort sein Kennwort ein.
//! „Bei privaten Adressen nicht pruefen" macht genau den Angriff moeglich,
//! gegen den TLS da ist.
//!
//! # Was statt dessen
//!
//! **Vertrauen beim ersten Mal, danach angeheftet** — das Modell von SSH.
//!
//! 1. Der Nutzer nennt die Adresse ausdruecklich (`set net.lan_devices`).
//!    Ohne diesen Schritt passiert nichts; die Vorgabe ist leer.
//! 2. Nur eine LITERALE private Adresse zaehlt, nie ein Name. Ein Name
//!    koennte beim zweiten Aufloesen woandershin zeigen; eine Adresse, die
//!    im URL steht, kann sich nicht verwandeln.
//! 3. Beim ersten Verbinden wird der Fingerabdruck des Blattzertifikats
//!    gemerkt. Ab dann muss es DASSELBE sein — ein anderes ist ein harter
//!    Fehler, auch wenn die Adresse freigegeben ist.
//!
//! Damit ist der Tausch benannt: **wir geben die Erstverbindung preis** (da
//! wissen wir nicht, mit wem wir reden), **und behalten alles danach**. Ein
//! Angreifer muss beim allerersten Kontakt schon dagestanden haben; wer
//! sich spaeter dazwischenschiebt, faellt auf.
//!
//! Nachgelassen werden auch nur die zwei Fehler, die ein echtes Geraet
//! zwangslaeufig ausloest. Abgelaufen, kaputt geparst, falsch signiert —
//! alles das bleibt toedlich.
//!
//! **Sitzungsgebunden.** Die Anheftung steht im RAM und ist nach einem
//! Neustart weg, genau wie der Keksbehaelter. Vertrauen auf Platte ist eine
//! eigene Entscheidung mit einer eigenen Diskussion.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::certstore::CertError;

/// Was gemerkt wurde: Adresse -> Fingerabdruck des Blattzertifikats.
static PINS: Mutex<Vec<(String, [u8; 32])>> = Mutex::new(Vec::new());

/// Ist `host` eine literale private Adresse, die der Nutzer freigegeben hat?
///
/// Zwei Bedingungen, beide noetig: die Adresse steht in `net.lan_devices`,
/// UND sie ist wirklich privat. Die zweite ist nicht Zierrat — sonst
/// koennte ein Tippfehler in der Konfiguration eine oeffentliche Adresse
/// freigeben, und der Nutzer saehe es nie.
fn is_allowed_device(host: &str) -> bool {
    let bare = host.split(':').next().unwrap_or(host);
    // NUR literale Adressen. Ein Name kaeme hier nie an derselben Stelle
    // heraus wie beim naechsten Aufloesen.
    let Some(ip) = crate::intent::parse_ip_pub(bare) else { return false };
    if crate::intent::reach::classify_ip(ip) == crate::intent::reach::Reach::Public {
        return false;
    }
    let list = crate::config::get("net.lan_devices").unwrap_or_default();
    list.split(',').map(str::trim).any(|e| !e.is_empty() && e == bare)
}

/// Darf dieser Fehler fuer dieses Geraet nachgelassen werden?
///
/// Nur die zwei, die ein selbstsigniertes Geraetezertifikat zwangslaeufig
/// ausloest. Alles andere heisst: mit diesem Zertifikat stimmt etwas, das
/// auch ein ehrliches Geraet nicht hat.
fn is_forgivable(e: CertError) -> bool {
    matches!(e, CertError::HostnameMismatch | CertError::UntrustedRoot)
}

/// Die zweite Chance. `Ok(())` heisst: durchlassen.
///
/// Gerufen NUR, wenn `verify_chain` schon nein gesagt hat — diese Funktion
/// kann nichts erlauben, was die richtige Pruefung erlaubt haette, und
/// nichts verbieten, was sie schon verboten hat.
pub fn second_chance(host: &str, leaf_der: &[u8], why: CertError) -> Result<(), CertError> {
    if !is_forgivable(why) || !is_allowed_device(host) {
        return Err(why);
    }
    let bare = host.split(':').next().unwrap_or(host);
    let fp = super::sha256::sha256(leaf_der);

    let mut pins = PINS.lock();
    if let Some((_, known)) = pins.iter().find(|(h, _)| h == bare) {
        if *known == fp {
            return Ok(());
        }
        // **Das ist der Fall, fuer den es die Anheftung gibt.** Die Adresse
        // ist freigegeben, aber jemand anders antwortet. Nicht durchlassen.
        crate::kprintln!(
            "[npk] LAN-ANHEFTUNG: {} zeigt ein ANDERES Zertifikat als beim ersten Mal.",
            bare);
        crate::kprintln!("[npk]   Das kann ein Geraetetausch sein — oder jemand dazwischen.");
        crate::kprintln!("[npk]   `set net.lan_devices` neu setzen loescht die Anheftung nicht;");
        crate::kprintln!("[npk]   dafuer braucht es einen Neustart. So ist es gemeint.");
        return Err(why);
    }

    // Erstkontakt. Hier wird das Vertrauen geschenkt, und genau hier ist es
    // ungedeckt — also sagt der Lauf es, statt es zu verschweigen.
    crate::kprintln!("[npk] LAN-Geraet {} beim ERSTEN Mal angenommen ({:?}).", bare, why);
    crate::kprintln!("[npk]   Fingerabdruck {:02x}{:02x}{:02x}{:02x}… ab jetzt angeheftet.",
        fp[0], fp[1], fp[2], fp[3]);
    pins.push((String::from(bare), fp));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nur die zwei Fehler, die ein ehrliches Geraet ausloest. Alles andere
    /// bleibt toedlich — auch mit freigegebener Adresse.
    #[test]
    fn only_the_two_unavoidable_errors_are_forgivable() {
        assert!(is_forgivable(CertError::HostnameMismatch));
        assert!(is_forgivable(CertError::UntrustedRoot));
        for e in [CertError::Expired, CertError::NotYetValid, CertError::ParseError,
                  CertError::SignatureInvalid, CertError::EmptyChain, CertError::NotCA,
                  CertError::KeyUsageInvalid, CertError::EkuInvalid,
                  CertError::PathLenExceeded, CertError::UnknownCriticalExt,
                  CertError::BadValidityDate] {
            assert!(!is_forgivable(e), "{e:?} darf nicht nachgelassen werden");
        }
    }
}
