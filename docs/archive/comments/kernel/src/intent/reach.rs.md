# `kernel/src/intent/reach.rs` @ 5e0102684

## L1-15 · `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]`

```
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
```

## L17-19 · `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]`

```
/// Wie offen ein Netzbereich ist. Die Ordnung ist der ganze Punkt:
/// `Local < Private < Public`, und eine Anfrage darf nie von offen nach
/// geschlossen laufen.
```

## L22-23 · `Local,`

```
/// Diese Maschine selbst — 127/8 und die Link-Local-Adressen, hinter
/// denen im Zweifel ein Geraetedienst sitzt.
```

## L25 · `Private,`

```
/// Das Netz des Nutzers: 10/8, 172.16/12, 192.168/16, 100.64/10.
```

## L27 · `Public,`

```
/// Das offene Internet.
```

## L31-35 · `pub fn classify_ip(ip: [u8; 4]) -> Reach {`

```
/// Der Bereich, in dem eine Adresse liegt.
///
/// **Entschieden wird an der ADRESSE, nie am Namen.** Ein Name kann beim
/// zweiten Aufloesen woandershin zeigen (DNS-Rebinding); eine Adresse kann
/// sich nicht verwandeln.
```

## L38-39 · `[0, ..] => Reach::Local,`

```
// 0.0.0.0/8 heisst „diese Maschine, dieses Netz" und wird von
// manchen Stapeln wie Loopback behandelt. Also die strengste Klasse.
```

## L42-43 · `[169, 254, ..] => Reach::Local,`

```
// Link-local: 169.254/16. Dort sitzt unter anderem der
// Metadatendienst jeder Cloud, und genau der ist das klassische Ziel.
```

## L48-50 · `[100, b, ..] if (64..=127).contains(&b) => Reach::Private,`

```
// Carrier-Grade NAT (100.64/10). Steht im offenen Netz nicht zur
// Verfuegung und ist fuer eine fremde Seite genauso interessant
// wie 10/8.
```

## L56-61 · `pub fn allows(from: Reach, to: Reach) -> bool {`

```
/// Darf ein Dokument der Klasse `from` eine Adresse der Klasse `to`
/// erreichen?
///
/// Die ganze Regel in einer Zeile: **nie von offen nach geschlossen.** Eine
/// oeffentliche Seite bleibt draussen; eine Seite aus dem Heimnetz darf ihr
/// eigenes Netz und das offene Internet; eine lokale darf alles.
```

## L86-88 · `#[test]`

```
/// Die NACHBARN der privaten Bereiche sind oeffentlich. Ein Off-by-one
/// hier ist eine Luecke, die niemand sieht: `172.15.x` und `172.32.x`
/// gehoeren dem offenen Netz, `100.63` und `100.128` auch.
```

## L116-117 · `#[test]`

```
/// Der Router-Fall, um den es praktisch geht: die Seite AUF dem Router
/// darf ihre eigenen Bilder laden.
```

## L124-125 · `#[test]`

```
/// Die Ordnung selbst — sie traegt die ganze Regel, also wird sie
/// geprueft und nicht angenommen.
```

