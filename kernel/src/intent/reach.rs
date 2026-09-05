//! Reichweite — darf eine Seite dorthin, wo sie hin will?
//!
//! Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2. Kurz: eine
//! oeffentliche Seite darf das private Netz des Nutzers nicht erreichen.
//! CORS deckt das NICHT ab — es schuetzt den Zielserver, nicht das Netz, in
//! dem der Browser steht. Browser haben die Regel als *Private Network
//! Access* nachgerueckt und bis heute nicht vollstaendig; wir bauen sie von
//! Anfang an.
//!
//! **Diese Datei haengt an NICHTS.** Kein `alloc`, kein `crate::`, nur
//! `core`. Das ist Absicht: der Kernel hat keine Testinfrastruktur, und eine
//! Sicherheitsregel, die man nicht fahren kann, ist eine Behauptung. So
//! mountet `beak-engine` sie in seinen Testbaum und faehrt die Tabelle unten
//! bei jedem `cargo test` mit — bei EINER Implementierung, nicht einer
//! Kopie ([[feedback-a-copy-is-a-second-semantics-waiting]]).

/// Wie offen ein Netzbereich ist. Die Ordnung ist der ganze Punkt:
/// `Local < Private < Public`, und eine Anfrage darf nie von offen nach
/// geschlossen laufen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// Diese Maschine selbst — 127/8 und die Link-Local-Adressen, hinter
    /// denen im Zweifel ein Geraetedienst sitzt.
    Local,
    /// Das Netz des Nutzers: 10/8, 172.16/12, 192.168/16, 100.64/10.
    Private,
    /// Das offene Internet.
    Public,
}

/// Der Bereich, in dem eine Adresse liegt.
///
/// **Entschieden wird an der ADRESSE, nie am Namen.** Ein Name kann beim
/// zweiten Aufloesen woandershin zeigen (DNS-Rebinding); eine Adresse kann
/// sich nicht verwandeln.
pub fn classify_ip(ip: [u8; 4]) -> Reach {
    match ip {
        // 0.0.0.0/8 heisst „diese Maschine, dieses Netz" und wird von
        // manchen Stapeln wie Loopback behandelt. Also die strengste Klasse.
        [0, ..] => Reach::Local,
        [127, ..] => Reach::Local,
        // Link-local: 169.254/16. Dort sitzt unter anderem der
        // Metadatendienst jeder Cloud, und genau der ist das klassische Ziel.
        [169, 254, ..] => Reach::Local,
        [10, ..] => Reach::Private,
        [172, b, ..] if (16..=31).contains(&b) => Reach::Private,
        [192, 168, ..] => Reach::Private,
        // Carrier-Grade NAT (100.64/10). Steht im offenen Netz nicht zur
        // Verfuegung und ist fuer eine fremde Seite genauso interessant
        // wie 10/8.
        [100, b, ..] if (64..=127).contains(&b) => Reach::Private,
        _ => Reach::Public,
    }
}

/// Darf ein Dokument der Klasse `from` eine Adresse der Klasse `to`
/// erreichen?
///
/// Die ganze Regel in einer Zeile: **nie von offen nach geschlossen.** Eine
/// oeffentliche Seite bleibt draussen; eine Seite aus dem Heimnetz darf ihr
/// eigenes Netz und das offene Internet; eine lokale darf alles.
pub fn allows(from: Reach, to: Reach) -> bool {
    to >= from
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_private_ranges_are_private() {
        for ip in [[10, 0, 0, 1], [10, 255, 255, 255], [192, 168, 1, 1],
                   [172, 16, 0, 1], [172, 31, 255, 254], [100, 64, 0, 1],
                   [100, 127, 255, 255]] {
            assert_eq!(classify_ip(ip), Reach::Private, "{ip:?}");
        }
    }

    #[test]
    fn the_local_ranges_are_local() {
        for ip in [[127, 0, 0, 1], [127, 1, 2, 3], [169, 254, 169, 254], [0, 0, 0, 0]] {
            assert_eq!(classify_ip(ip), Reach::Local, "{ip:?}");
        }
    }

    /// Die NACHBARN der privaten Bereiche sind oeffentlich. Ein Off-by-one
    /// hier ist eine Luecke, die niemand sieht: `172.15.x` und `172.32.x`
    /// gehoeren dem offenen Netz, `100.63` und `100.128` auch.
    #[test]
    fn the_neighbours_of_the_private_ranges_are_public() {
        for ip in [[9, 255, 255, 255], [11, 0, 0, 1],
                   [172, 15, 0, 1], [172, 32, 0, 1],
                   [192, 167, 0, 1], [192, 169, 0, 1],
                   [100, 63, 255, 255], [100, 128, 0, 1],
                   [126, 0, 0, 1], [128, 0, 0, 1],
                   [169, 253, 0, 1], [169, 255, 0, 1],
                   [8, 8, 8, 8], [1, 1, 1, 1]] {
            assert_eq!(classify_ip(ip), Reach::Public, "{ip:?}");
        }
    }

    #[test]
    fn a_public_page_never_reaches_inward() {
        assert!(!allows(Reach::Public, Reach::Private));
        assert!(!allows(Reach::Public, Reach::Local));
        assert!(allows(Reach::Public, Reach::Public));
    }

    #[test]
    fn a_page_may_always_reach_outward() {
        assert!(allows(Reach::Private, Reach::Public));
        assert!(allows(Reach::Local, Reach::Public));
        assert!(allows(Reach::Local, Reach::Private));
    }

    /// Der Router-Fall, um den es praktisch geht: die Seite AUF dem Router
    /// darf ihre eigenen Bilder laden.
    #[test]
    fn the_router_page_may_load_its_own_assets() {
        assert!(allows(Reach::Private, Reach::Private));
        assert!(allows(Reach::Local, Reach::Local));
    }

    /// Die Ordnung selbst — sie traegt die ganze Regel, also wird sie
    /// geprueft und nicht angenommen.
    #[test]
    fn the_ordering_is_the_rule() {
        assert!(Reach::Local < Reach::Private);
        assert!(Reach::Private < Reach::Public);
    }
}
