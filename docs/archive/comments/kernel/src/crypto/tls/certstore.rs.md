# `kernel/src/crypto/tls/certstore.rs` @ 5e0102684

## L1-10 · `use alloc::vec::Vec;`

```
//! Certificate Store
//!
//! Trusted root CA anchors + chain validation.
//!
//! Two tiers, deliberately: a built-in floor compiled into the signed
//! kernel, and a store of DER files under `sys/certs/` that arrives as
//! signed OTA assets or is added by hand. The floor exists so a broken,
//! empty or hostile store can never cut the machine off from its own
//! updates — the anchors needed to reach the update host are code, and
//! code cannot go missing. Everything above that is data.
```

## L19 · `const ISRG_ROOT_X1_DER: &[u8] = include_bytes!("../../../certs/isrg_root_x1.der");`

```
/// ISRG Root X1 (Let's Encrypt) — covers ~60% of the web
```

## L22 · `const DIGICERT_GLOBAL_G2_DER: &[u8] = include_bytes!("../../../certs/digicert_global_g2.der");`

```
/// DigiCert Global Root G2 — covers Anthropic, Cloudflare, etc.
```

## L25 · `const AAA_CERT_SERVICES_DER: &[u8] = include_bytes!("../../../certs/aaa_certificate_services.der");`

```
/// AAA Certificate Services (Comodo/Sectigo) — covers Cloudflare default certs
```

## L28 · `const GTS_ROOT_R1_DER: &[u8] = include_bytes!("../../../certs/gts_root_r1.der");`

```
/// Google Trust Services Root R1 — covers Google services
```

## L31-38 · `const USERTRUST_ECC_DER: &[u8] = include_bytes!("../../../certs/usertrust_ecc.der");`

```
/// USERTrust ECC Certification Authority — Sectigo's modern ECC root.
/// Sectigo cross-signs newer roots (Public Server Authentication Root
/// E46) under USERTrust ECC, so adding the cross-anchor here covers
/// github.com + most Sectigo-issued ECDSA certs in 2025+.
///
/// **"Covers" only as far as the SERVER cooperates** — see the two Sectigo
/// roots below. That sentence was written from the certificate's structure,
/// and the structure is only half the question.
```

## L41-43 · `const USERTRUST_RSA_DER: &[u8] = include_bytes!("../../../certs/usertrust_rsa.der");`

```
/// USERTrust RSA Certification Authority — Sectigo's modern RSA root.
/// Counterpart to USERTrust ECC for RSA chains. Covers Sectigo
/// Public Server Authentication Root R46 + a wide RSA customer base.
```

## L46-48 · `const AMAZON_ROOT_CA1_DER: &[u8] = include_bytes!("../../../certs/amazon_root_ca1.der");`

```
/// Amazon Root CA 1 — anchors CloudFront, which fronts a large share of
/// the web (doc.rust-lang.org among them). Its absence was measured, not
/// guessed: those sites failed with `certificate: untrusted root CA`.
```

## L51-53 · `const ISRG_ROOT_X2_DER: &[u8] = include_bytes!("../../../certs/isrg_root_x2.der");`

```
/// ISRG Root X2 — Let's Encrypt's ECDSA hierarchy, a separate anchor from
/// X1. Servers that chain to X2 rather than offering an X1-anchored
/// variant were unreachable with X1 alone.
```

## L56-65 · `const GTS_ROOT_R4_DER: &[u8] = include_bytes!("../../../certs/gts_root_r4.der");`

```
/// GTS Root R4 — Google Trust Services' ECDSA root, and the counterpart to
/// R1 in exactly the way X2 is to X1: R1 anchors Google's RSA
/// intermediates (WR1/WR2), R4 their ECDSA ones (WE1/WE2). Having only R1
/// was not "most of Google", it was "none of the ECDSA half".
///
/// **Cloudflare's default certificates now come from GTS**, so this is not
/// a Google-only anchor: measured over 38 real hosts, R4 is what
/// `cdnjs.cloudflare.com`, `unpkg.com` and `cdn.fonts.net` chain to — two
/// of the most-linked script CDNs on the web. arcade.ch stylesheet import
/// died on exactly this: `TLS error: certificate: untrusted root CA`.
```

## L68-70 · `const GLOBALSIGN_ROOT_R3_DER: &[u8] = include_bytes!("../../../certs/globalsign_root_r3.der");`

```
/// GlobalSign Root CA - R3 — anchors `crates.io` (GlobalSign Atlas) and
/// orf.at. RSA 2048, and it EXPIRES 2029-03-18: the earliest expiry in this
/// list by six years, so it is the first one to come back to.
```

## L73-76 · `const SWISSSIGN_RSA_2022_DER: &[u8] = include_bytes!("../../../certs/swisssign_rsa_2022.der");`

```
/// SwissSign RSA TLS Root CA 2022 - 1 — the Swiss Post's anchor, and with
/// it a good part of Swiss public-sector TLS. In the same measurement it
/// was the only anchor no other host shared, which is precisely why it
/// would never have been guessed.
```

## L79-81 · `const DIGICERT_GLOBAL_G3_DER: &[u8] = include_bytes!("../../../certs/digicert_global_g3.der");`

```
/// DigiCert Global Root G3 — the ECDSA twin of G2, and the third time the
/// same shape bit: akamai, blick.ch, credit-suisse.com, faz.net and
/// DigiCert's own OCSP responder all chain here, none of them to G2.
```

## L84-85 · `const GLOBALSIGN_ROOT_R46_DER: &[u8] = include_bytes!("../../../certs/globalsign_root_r46.der");`

```
/// GlobalSign Root R46 — GlobalSign's 2019 RSA root, a different anchor
/// from the 2009 "Root CA - R3" above. bbc.co.uk, europa.eu, theguardian.com.
```

## L88-89 · `const GLOBALSIGN_ROOT_E46_DER: &[u8] = include_bytes!("../../../certs/globalsign_root_e46.der");`

```
/// GlobalSign Root E46 — the ECDSA twin of R46. See the note on twins below:
/// this one is INFERRED, not measured.
```

## L92-93 · `const STARFIELD_G2_DER: &[u8] = include_bytes!("../../../certs/starfield_g2.der");`

```
/// Starfield Root Certificate Authority - G2 — GoDaddy/Starfield, and with
/// it Fastly's certificates.
```

## L96-97 · `const DIGICERT_TLS_RSA4096_G5_DER: &[u8] = include_bytes!("../../../certs/digicert_tls_rsa4096_g5.der");`

```
/// DigiCert TLS RSA4096 Root G5 — DigiCert's 2021 hierarchy, separate from
/// the Global Root G2/G3 pair. raiffeisen.ch.
```

## L100 · `const DIGICERT_TLS_ECC_P384_G5_DER: &[u8] = include_bytes!("../../../certs/digicert_tls_ecc_p384_g5.der");`

```
/// DigiCert TLS ECC P384 Root G5 — the ECDSA twin of RSA4096 G5. INFERRED.
```

## L103-104 · `const TTELESEC_GLOBALROOT_CLASS2_DER: &[u8] = include_bytes!("../../../certs/ttelesec_globalroot_class2.der");`

```
/// T-TeleSec GlobalRoot Class 2 — Deutsche Telekom, and with it a good part
/// of German public-sector TLS (bundesbank.de measured).
```

## L107-109 · `const HARICA_TLS_RSA_2021_DER: &[u8] = include_bytes!("../../../certs/harica_tls_rsa_2021.der");`

```
/// HARICA TLS RSA Root CA 2021 — the Greek academic CA, which is what
/// bund.de chains to. Named here because nobody would have guessed it:
/// a German federal portal on a Greek university's root.
```

## L112-114 · `const AMAZON_ROOT_CA2_DER: &[u8] = include_bytes!("../../../certs/amazon_root_ca2.der");`

```
/// Amazon Root CA 2/3/4 — the rest of the family around CA 1. CA 3 is
/// measured (telekom.de); 2 and 4 complete the RSA-4096 / ECDSA-P384 pair
/// the same way, see the note on twins below.
```

## L119-120 · `const IDENTRUST_COMMERCIAL_CA1_DER: &[u8] = include_bytes!("../../../certs/identrust_commercial_root_ca1.der");`

```
/// IdenTrust Commercial Root CA 1 — its own hierarchy, not only the
/// DST-Root cross-sign people remember it for. ing.de, identrust.com.
```

## L123-127 · `const DTRUST_CLASS3_EV_2009_DER: &[u8] = include_bytes!("../../../certs/dtrust_root_class3_ca2_ev_2009.der");`

```
/// D-TRUST (Bundesdruckerei) — German public-sector TLS, and it takes TWO
/// anchors because the hierarchy was renewed: the 2009 EV root carries
/// elster.de (the German tax portal), the 2023 BR root carries
/// bsi.bund.de — the federal office for information security itself.
/// The 2009 one expires 2029-11-05.
```

## L131-135 · `const CERTUM_TRUSTED_ROOT_DER: &[u8] = include_bytes!("../../../certs/certum_trusted_root_ca.der");`

```
/// Certum (Asseco, PL), Buypass (NO) and Actalis (IT) — three European CAs
/// with a national customer base each. Each was found on its own site only,
/// which is weak evidence on its own; they are here because a European
/// desktop that cannot open a Polish, Norwegian or Italian government or
/// bank page is not finished, and an anchor costs ~1.4 KB.
```

## L140-154 · `const SECTIGO_PSA_E46_DER: &[u8] = include_bytes!("../../../certs/sectigo_public_server_e46.der");`

```
/// Sectigo Public Server Authentication Root E46 / R46 — DIRECTLY, although
/// USERTrust ECC/RSA cross-sign them and the note above said that covers it.
///
/// **Eine Kreuzsignatur hilft nur, wenn der SERVER ihren Pfad mitliefert.**
/// Der Anker ist da, aber die Kette dorthin baut nicht der Client, sondern
/// der Server aus dem, was er schickt. github.com und code.jquery.com
/// liefern den Weg ueber USERTrust und gingen deshalb durch;
/// `www.dkb.de` schickt genau zwei Karten — sein Blatt und
/// "Public Server Authentication CA EV E36" — und deren Aussteller ist Root
/// E46 und sonst nichts. Ohne diesen Anker: "unable to get local issuer
/// certificate", auf der Anmeldeseite einer Bank.
///
/// Gefunden hat es NICHT die Aufzaehlung der Aussteller (die nannte E46, und
/// E46 galt als abgedeckt), sondern erst der Lauf, der den GANZEN Boden als
/// einzigen Speicher gegen alle Wirte hielt.
```

## L158-162 · `const ROOT_CERTS: &[&[u8]] = &[`

```
/// Built-in anchors. This set is the FLOOR: it ships inside the signed
/// kernel, cannot be removed by an update or by the user, and is what
/// guarantees the update host stays reachable even when the npkFS store
/// is empty, stale, or broken. Everything else is delivered as data —
/// see [`store_roots`].
```

## L196-235 · `pub const STORE_DIR: &str = "sys/certs";`

```
// **Wie diese Liste entstanden ist — nicht geraten, ausgezaehlt.** Fuer 88
// echte Wirte (der Zielkorpus, die Skript- und Schriften-CDNs, die er
// verlinkt, dazu Schweizer und deutsche Behoerden, Banken, Zeitungen und die
// grossen Paketspeicher) wurde der ANKER bestimmt, den OpenSSL WIRKLICH
// benutzt: die hoechste `depth=`-Zeile. **Nicht die letzte Karte der
// gelieferten Kette** — die ist meistens ein Zwischenzertifikat, und wer sie
// nimmt, traegt Namen wie „DigiCert Global G2 TLS RSA SHA256 2020 CA1" in
// eine Wurzelliste ein. Danach jede neue Wurzel EINZELN als `-CAfile` gegen
// ihre Wirte gehalten, mit `-no-CApath`: das beweist die Kette, statt sich
// auf einen Fingerabdruck aus dem Gedaechtnis zu verlassen. 18 von 18 gruen.
//
// **Das Muster, das dabei dreimal dasselbe war: jede grosse CA fuehrt eine
// RSA- und eine ECDSA-Wurzel, und hier stand immer nur eine von beiden.**
// ISRG X1 ohne X2 (schon einmal nachgetragen), GTS R1 ohne R4, DigiCert G2
// ohne G3. Es ist kein Zufall und keine Reihe von Einzelfaellen: die eine
// Wurzel zu haben heisst nicht „die meisten Server dieser CA", sondern „die
// Haelfte" — und WELCHE Haelfte entscheidet der Server, nicht wir. Deshalb
// stehen zwei Anker hier, die NICHT gemessen wurden, sondern gefolgert:
// `GLOBALSIGN_ROOT_E46` und `DIGICERT_TLS_ECC_P384_G5`, die ECDSA-Zwillinge
// zweier Wurzeln, die gemessen gebraucht werden. Beide sind als gefolgert
// markiert, damit die naechste Messung sie bestaetigen oder wegwerfen kann.
//
// `AAA_CERT_SERVICES` traf in beiden Laeufen KEINEN Wirt mehr — Cloudflares
// alte Vorgabewurzel. Sie bleibt trotzdem: der Boden ist da, um erreichbar zu
// sein, nicht um knapp zu sein, und eine Wurzel zu ENTFERNEN ist eine
// Entscheidung fuer Geraete im Feld, nicht fuer diese Messung.
//
// **Der dritte Lauf ging absichtlich auf den SCHWANZ** — 30 Wirte, ausgesucht
// nach Ausstellern, die in den ersten 88 gar nicht vorkamen: Behoerden,
// Banken und Anbieter in DE/CH/PL/NO/IT/EE. Er fand neun weitere Anker, und
// zwei davon sind der Grund, warum ein Zensus des Schwanzes sein muss:
// `www.bsi.bund.de` — das Bundesamt fuer Sicherheit in der
// Informationstechnik — und `www.elster.de` haengen an D-TRUST, das in
// keiner CDN-Messung der Welt auftaucht. Das Muster oben schlug dabei ein
// VIERTES Mal zu: Amazon Root CA 1 war da, CA 3 (ECDSA) nicht, und
// telekom.de haengt an CA 3.
//
// **Was als naechstes ablaeuft:** `GLOBALSIGN_ROOT_R3` am 2029-03-18, dann
// `DTRUST_CLASS3_EV_2009` am 2029-11-05. Wer nach 2029 liest: crates.io und
// orf.at hingen am ersten, elster.de am zweiten.
```

## L237-240 · `pub const STORE_DIR: &str = "sys/certs";`

```
/// npkFS directory holding the data-delivered anchors. Off limits to WASM
/// apps — write access here is the power to mint a MITM anchor for the
/// whole system, so the guard that protects the module store covers this
/// path too (`wasm.rs::is_trust_critical_path`).
```

## L243-244 · `const MAX_STORE_ROOTS: usize = 64;`

```
/// A cap on the store, so a corrupt or hostile directory cannot exhaust
/// kernel memory during the boot load.
```

## L248-253 · `static STORE_ROOTS: Mutex<Vec<StoreRoot>> = Mutex::new(Vec::new());`

```
/// Anchors loaded from [`STORE_DIR`].
///
/// Held in memory and refreshed explicitly, never read from npkFS during a
/// handshake: `verify_chain` runs mid-TLS, and a handshake can be raised
/// while an npkFS write is in flight, so touching the filesystem from here
/// would buy a lock-order problem for nothing.
```

## L261-267 · `pub fn load_store() -> usize {`

```
/// (Re)load `sys/certs/` into the in-memory anchor set. Call after
/// `npkfs::mount`, and after anything changes that directory (`cert
/// add`/`remove`, an asset update). Returns how many anchors are live.
///
/// A file that does not parse as X.509 is skipped with a log line rather
/// than failing the load — one bad file must not take the rest of the
/// store down with it.
```

## L271-272 · `Ok(None) => { STORE_ROOTS.lock().clear(); return 0; }`

```
// No directory yet is the normal state on a fresh install, not an
// error: the built-in floor carries the machine until assets land.
```

## L298-300 · `if x509::parse_x509(&der).is_none() {`

```
// Parse before trusting: an anchor that cannot be read is an anchor
// that would silently never match, which looks like a network fault
// three layers up.
```

## L316 · `pub fn builtin_count() -> usize { ROOT_CERTS.len() }`

```
/// Number of built-in anchors — the floor that cannot be removed.
```

## L319 · `pub struct CertInfo {`

```
/// What an anchor actually is, for showing a human before they trust it.
```

## L323 · `pub not_before: Option<u64>,`

```
/// UTC seconds, or None when the date could not be decoded.
```

## L327 · `pub self_signed: bool,`

```
/// Subject == issuer AND the signature verifies against its own key.
```

## L332 · `pub fn describe(der: &[u8]) -> Option<CertInfo> {`

```
/// Describe a DER certificate. `None` if it does not parse as X.509.
```

## L347-348 · `pub fn fingerprint_hex(fp: &[u8; 32]) -> String {`

```
/// Lowercase hex SHA-256, colon-separated — the form CAs publish, so it
/// can be compared against the vendor's page character by character.
```

## L360-361 · `pub fn for_each_anchor(mut f: impl FnMut(Option<&str>, &[u8])) {`

```
/// Run `f` over every anchor: built-ins first (as `None`), then the stored
/// ones with their filename. Used by `cert list` to show provenance.
```

## L371-373 · `pub fn verify_chain(chain: &[&[u8]], hostname: &str) -> Result<(), CertError> {`

```
/// Verify a certificate chain.
/// `chain` is ordered leaf-first: [leaf, intermediate, ...].
/// Returns Ok(()) if the chain validates to a trusted root.
```

## L379 · `let leaf = x509::parse_x509(chain[0]).ok_or(CertError::ParseError)?;`

```
// Parse leaf certificate
```

## L382-384 · `let now = now_unix();`

```
// Wall clock, or None when nothing believable is available. Resolved
// ONCE for the whole chain so a tick between two certs cannot make the
// verdict depend on where in the chain the check happened to land.
```

## L390 · `if !cn_matches(&leaf, hostname) {`

```
// Hostname → CN/SAN match
```

## L394 · `if leaf.unknown_critical_ext {`

```
// Critical extension we don't understand → RFC 5280 §4.2 reject.
```

## L398 · `if let Some(ku) = leaf.key_usage {`

```
// KeyUsage: if present, must include digitalSignature (TLS 1.3 ECDHE_*).
```

## L404 · `if leaf.eku_present && !leaf.eku_server_auth && !leaf.eku_any {`

```
// EKU: if present, must include serverAuth or anyExtendedKeyUsage.
```

## L409-412 · `let mut current = leaf;`

```
// Build chain: verify each cert is signed by the next.
// For each issuer (CA), enforce CA-bit, KU keyCertSign, pathLen, and the
// critical-extension rule. `inter_below` counts non-self CAs that the
// current issuer sits above in the chain (excluding the leaf).
```

## L421 · `if !issuer.is_ca {`

```
// Issuer must assert CA via BasicConstraints.
```

## L425 · `if let Some(ku) = issuer.key_usage {`

```
// KeyUsage on a CA, if present, must include keyCertSign.
```

## L431-433 · `if let Some(plc) = issuer.path_len_constraint {`

```
// pathLenConstraint applies to non-self-issued certs below this CA in
// the chain (RFC 5280 §4.2.1.9). `i - 1` is the count of intermediate
// CAs sitting between this issuer and the leaf.
```

## L443-447 · `if let Some(now) = now {`

```
// Intermediates are checked like the leaf. The trust ANCHOR is not:
// a root is trusted by its key, and browsers deliberately do not
// fail a chain over an anchor's own dates — otherwise a root aging
// out would break every site under it even after the replacement
// has been cross-signed.
```

## L455-457 · `for root_der in ROOT_CERTS {`

```
// The top of the chain must resolve to a trusted anchor. Built-in floor
// first, then the data-delivered store — one shared test, so a stored
// anchor is never held to a weaker standard than a compiled-in one.
```

## L472-479 · `const CLOCK_SANE_FLOOR: u64 = 1_735_689_600;`

```
// ── Validity dates ────────────────────────────────────────────────────
//
// Below this, the clock is not believable and the check is SKIPPED rather
// than enforced. A dead CMOS battery reads the year 2000; enforcing
// against that would reject every certificate on earth and take HTTPS
// down completely — a far worse failure than honouring a stale one. The
// floor only has to be late enough that a plausible clock is a useful
// clock: 2025-01-01.
```

## L482-483 · `fn now_unix() -> Option<u64> {`

```
/// Current UTC seconds, or `None` when no source is trustworthy.
/// NTP first — it is the accurate one; CMOS is the offline fallback.
```

## L490-494 · `fn parse_asn1_time(v: &[u8]) -> Option<u64> {`

```
/// Decode a DER ASN.1 time into UTC seconds.
///
/// The two encodings are told apart by length, which is sound because DER
/// pins the form (RFC 5280 §4.1.2.5 — seconds mandatory, always `Z`):
/// UTCTime is `YYMMDDHHMMSSZ` (13), GeneralizedTime `YYYYMMDDHHMMSSZ` (15).
```

## L507 · `let yy = num(&v[0..2])?;`

```
// RFC 5280 §4.1.2.5.1: YY >= 50 means 19YY, below means 20YY.
```

## L531-532 · `fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {`

```
/// Days since the Unix epoch for a civil date (Howard Hinnant's
/// `days_from_civil`, valid across the whole proleptic Gregorian range).
```

## L543-544 · `fn check_validity(cert: &X509Cert, now: u64) -> Result<(), CertError> {`

```
/// Check one certificate against `now`. A date we cannot decode is a
/// reason to reject: an unreadable validity period is not a valid one.
```

## L557 · `fn anchors_chain(current: &X509Cert, root_der: &[u8]) -> bool {`

```
/// Does `root_der` anchor a chain whose topmost cert is `current`?
```

## L564 · `if current.issuer_cn == root.subject_cn && verify_signature(current, &root) {`

```
// Check if current cert's issuer matches root's subject
```

## L569-577 · `if current.subject_cn == root.subject_cn {`

```
// The last cert IS one of our trusted roots. Match it by IDENTITY —
// same subject + same public key — NOT by verifying its own signature.
// This is required for cross-signed roots: e.g. google.* now serves GTS
// Root R1 cross-signed by GlobalSign Root CA (issuer != subject), so its
// self-signature check fails against GTS R1's own key even though the key
// IS our anchor. The chain up to `current` was already signature-verified
// by the caller, and an anchor is trusted by its key (RFC 5280 §6.1 trust
// anchor), so matching the embedded key is sufficient and correct. The
// `verify_signature` arm keeps the classic self-signed path.
```

## L590-595 · `const OID_ECDSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];`

```
// Signature algorithm OIDs — SHA-256 and SHA-384 only.
// SHA-1 (`1.2.840.113549.1.1.5`) is rejected: collision-broken since 2017,
// last accepted by mainstream CAs ~2016. We never verify root self-signatures
// (roots are matched by subject DN against the embedded set), so SHA-1 only
// matters for intermediate/leaf chain hops — and there it's a hard reject.
// 1.2.840.10045.4.3.2 = ecdsa-with-SHA256
```

## L597 · `const OID_ECDSA_SHA384: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x03];`

```
// 1.2.840.10045.4.3.3 = ecdsa-with-SHA384
```

## L599 · `const OID_RSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B];`

```
// 1.2.840.113549.1.1.11 = sha256WithRSAEncryption
```

## L601 · `const OID_RSA_SHA384: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0C];`

```
// 1.2.840.113549.1.1.12 = sha384WithRSAEncryption
```

## L636 · `fn ecdsa_p256_verify_sha256(pubkey: &[u8], tbs: &[u8], signature: &[u8]) -> bool {`

```
/// ECDSA P-256 verify with SHA-256 digest.
```

## L659 · `fn ecdsa_p384_verify_sha384(pubkey: &[u8], tbs: &[u8], signature: &[u8]) -> bool {`

```
/// ECDSA P-384 verify with SHA-384 digest.
```

## L682-686 · `pub fn verify_p384_sha384(pubkey: &[u8], data: &[u8], signature: &[u8]) -> bool {`

```
/// Verify an ECDSA P-384 signature over raw data.
/// Computes SHA-384 ourselves, then uses PrehashVerifier (proven path on bare metal).
/// pubkey: 97-byte uncompressed SEC1 point.
/// data: the raw data that was signed.
/// signature: DER-encoded ECDSA signature.
```

## L692-693 · `pub fn verify_p384_prehash_384(pubkey: &[u8], prehash: &[u8; 48], signature: &[u8]) -> bool {`

```
/// Verify an ECDSA P-384 signature over a pre-computed SHA-384 digest.
/// Same path as TLS cert verification — proven on bare metal.
```

## L714-722 · `pub fn covers(leaf_der: &[u8], hostname: &str) -> bool {`

```
/// Deckt dieses Blattzertifikat auch DIESEN Namen?
///
/// Fuer Connection Coalescing (RFC 7540 §9.1.1): eine schon aufgebaute
/// Verbindung darf einen zweiten Namen bedienen, wenn sie zur selben Adresse
/// geht UND das Zertifikat den Namen deckt. Der Rest der Kette wurde beim
/// Handshake geprueft und aendert sich nicht — nur der Name ist neu, also ist
/// der Name die einzige Frage. Bewusst DIESELBE Funktion wie im Handshake:
/// zwei Namenspruefungen nebeneinander laufen auseinander, und die schwaechere
/// gewinnt dann immer.
```

## L731 · `let cn = core::str::from_utf8(cert.subject_cn).unwrap_or("");`

```
// Check CN first
```

## L737 · `if let Some(sans) = extract_sans(cert.tbs_raw) {`

```
// Check SANs in TBS raw bytes (OID 2.5.29.17 = subjectAltName)
```

## L751 · `fn name_matches(name: &str, hostname: &str) -> bool {`

```
/// Check if a certificate name (CN or SAN) matches the hostname.
```

## L756 · `if let Some(wildcard_domain) = name.strip_prefix("*.") {`

```
// Wildcard: *.example.com matches foo.example.com
```

## L767 · `const OID_SAN: &[u8] = &[0x55, 0x1D, 0x11];`

```
// OID 2.5.29.17 = subjectAltName
```

## L770 · `fn extract_sans(tbs: &[u8]) -> Option<&[u8]> {`

```
/// Search TBS bytes for the SAN extension and return the inner SEQUENCE bytes.
```

## L772 · `for i in 0..tbs.len().saturating_sub(OID_SAN.len() + 4) {`

```
// Scan for OID_SAN pattern in DER bytes
```

## L775 · `let mut pos = i + OID_SAN.len();`

```
// After OID, skip to the OCTET STRING containing the SAN SEQUENCE
```

## L777 · `while pos < tbs.len() {`

```
// There may be a BOOLEAN (critical) before the OCTET STRING
```

## L780 · `if tag == 0x04 { // OCTET STRING`

```
// OCTET STRING
```

## L788 · `} else if tag == 0x01 { // BOOLEAN (critical flag)`

```
// BOOLEAN (critical flag)
```

## L814 · `struct SanIter<'a> {`

```
/// Iterator over DNS names in a SAN extension (tag 0x82 = dNSName).
```

## L822 · `let mut pos = 0;`

```
// Skip outer SEQUENCE tag if present
```

## L846 · `if tag == 0x82 {`

```
// Tag 0x82 = context-specific [2] = dNSName
```

## L855-856 · `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`

```
// `Copy`, damit ein Fehler weitergereicht werden kann, ohne ihn zu
// verbrauchen: `lanpin::second_chance` muss ihn pruefen UND zurueckgeben.
```

## L875-878 · `pub fn reason(&self) -> &'static str {`

```
/// A stable, static reason string. Static because the whole HTTP layer
/// carries `&'static str` errors — that is what lets the real cause
/// travel from here up to the browser instead of being flattened into
/// "TLS handshake failed" at the first boundary.
```

