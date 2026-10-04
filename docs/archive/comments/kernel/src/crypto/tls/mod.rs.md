# `kernel/src/crypto/tls/mod.rs` @ 5e0102684

## L1-5 · `pub mod sha256;`

```
//! TLS 1.3 (RFC 8446)
//!
//! Minimal implementation for HTTPS client connections.
//! Cipher suites: TLS_AES_128_GCM_SHA256, TLS_AES_256_GCM_SHA384,
//!                TLS_CHACHA20_POLY1305_SHA256
```

## L14 · `pub mod lanpin;`

```
/// Geraete im eigenen Netz — angeheftetes Vertrauen statt „ignorieren".
```

## L22 · `const TLS_VERSION_12: [u8; 2] = [0x03, 0x03]; // Record layer uses 1.2`

```
// TLS 1.3 constants
```

## L23 · `const TLS_VERSION_12: [u8; 2] = [0x03, 0x03]; // Record layer uses 1.2`

```
// Record layer uses 1.2
```

## L24 · `const TLS_VERSION_13: [u8; 2] = [0x03, 0x04]; // Supported versions extension`

```
// Supported versions extension
```

## L26 · `const CT_CHANGE_CIPHER_SPEC: u8 = 0x14;`

```
// Content types
```

## L32 · `const HT_CLIENT_HELLO: u8 = 0x01;`

```
// Handshake types
```

## L40 · `const EXT_SERVER_NAME: u16 = 0x0000;`

```
// Extension types
```

## L48 · `const GROUP_SECP384R1: u16 = 0x0018;`

```
// Named groups
```

## L54 · `Secp384r1(Vec<u8>), // 97 bytes uncompressed point`

```
// 97 bytes uncompressed point
```

## L57 · `const MAX_RECORD_PAYLOAD: usize = 16384 + 256; // 16KB + overhead`

```
// Max TLS record payload
```

## L58 · `const MAX_RECORD_PAYLOAD: usize = 16384 + 256; // 16KB + overhead`

```
// 16KB + overhead
```

## L60-62 · `#[derive(Clone, Copy, PartialEq)]`

```
// ============================================================
// Cipher Suite
// ============================================================
```

## L66 · `Aes128Gcm,         // TLS_AES_128_GCM_SHA256 (0x1301)`

```
// TLS_AES_128_GCM_SHA256 (0x1301)
```

## L67 · `Aes256Gcm,         // TLS_AES_256_GCM_SHA384 (0x1302)`

```
// TLS_AES_256_GCM_SHA384 (0x1302)
```

## L68 · `Chacha20Poly1305,  // TLS_CHACHA20_POLY1305_SHA256 (0x1303)`

```
// TLS_CHACHA20_POLY1305_SHA256 (0x1303)
```

## L95-97 · `#[derive(Clone)]`

```
// ============================================================
// Transcript Hash (SHA-256 or SHA-384 depending on cipher suite)
// ============================================================
```

## L128-130 · `fn tls_aead_encrypt(cs: CipherSuite, key: &[u8], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {`

```
// ============================================================
// AEAD dispatch (ChaCha20-Poly1305, AES-128-GCM, AES-256-GCM)
// ============================================================
```

## L178-180 · `fn ks_empty_hash(cs: CipherSuite) -> Vec<u8> {`

```
// ============================================================
// Key Schedule dispatch (SHA-256 or SHA-384)
// ============================================================
```

## L272-274 · `struct P384KeyPair {`

```
// ============================================================
// P-384 ECDH Key Exchange
// ============================================================
```

## L278 · `public_uncompressed: [u8; 97], // 0x04 || x(48) || y(48)`

```
// 0x04 || x(48) || y(48)
```

## L282 · `let r1 = csprng::random_256();`

```
// Generate 48 random bytes from CSPRNG
```

## L289 · `let secret = loop {`

```
// Retry if invalid (zero or >= curve order)
```

## L294 · `let r = csprng::random_256();`

```
// Reseed and retry (extremely unlikely)
```

## L299 · `use p384::elliptic_curve::sec1::ToEncodedPoint;`

```
// Compute public key (uncompressed point)
```

## L325-327 · `pub struct TlsSession {`

```
// ============================================================
// TLS Session
// ============================================================
```

## L332 · `client_app_key: [u8; 32], // first cipher.key_len() bytes used`

```
// first cipher.key_len() bytes used
```

## L338-340 · `alpn: Option<String>,`

```
/// Protocol the server picked from our ALPN offer (RFC 7301). `None` when
/// we offered nothing or the server stayed silent — then HTTP/1.1 is
/// implied, since that is what an ALPN-less connection has always meant.
```

## L342-350 · `leaf_der: Vec<u8>,`

```
/// Das Blattzertifikat der Gegenstelle, roh. Aufgehoben fuer genau eine
/// Frage: deckt es auch einen ZWEITEN Namen? Das ist die Bedingung fuers
/// Coalescing (RFC 7540 §9.1.1) — `de.wikipedia.org`,
/// `thumb.wikimedia.org` und `auth.wikimedia.org` liegen auf derselben
/// Adresse und auf demselben Zertifikat, und wir bauen zu jedem einzeln
/// auf: gemessen 3 x 90 ms je Seitenaufbau.
///
/// Roh und nicht als Namensliste, damit die Frage von derselben Funktion
/// beantwortet wird wie beim Handshake.
```

## L352 · `peer: ([u8; 4], u16),`

```
/// Wohin diese Verbindung geht. Der erste Teil derselben Bedingung.
```

## L354-363 · `rx: Vec<u8>,`

```
/// Halb angekommene Bytes eines Satzes, fuer den NICHT-blockierenden
/// Leser (`tls_poll`).
///
/// **Der Unterschied zwischen Holen und Lauschen.** `tls_recv` wartet auf
/// einen Satz — richtig fuer eine Antwort, die man angefordert hat, und
/// falsch fuer eine Verbindung, die meistens still ist und irgendwann von
/// selbst etwas sagt. Ein WebSocket ist der zweite Fall: ein Browser
/// fragt ihn in JEDEM Bild, und zehn Millisekunden Warten je Bild waeren
/// der Preis fuer nichts. Also sammeln statt warten — was ankommt, liegt
/// hier, bis ein Satz vollstaendig ist.
```

## L365-368 · `rx_plain: Vec<u8>,`

```
/// Der ENTSCHLUESSELTE Rest. Ein Satz ist bis zu 16 KB gross, der Puffer
/// des Rufers darf kleiner sein — und der Satzzaehler rueckt beim
/// Entschluesseln vor, ein weggeworfener Klartext waere unwiederbringlich.
/// Also wird hier abgelegt, was diesmal nicht mehr hineinpasst.
```

## L377 · `pub fn alpn(&self) -> Option<&str> {`

```
/// The negotiated ALPN protocol, e.g. `"h2"` or `"http/1.1"`.
```

## L382 · `pub fn peer(&self) -> ([u8; 4], u16) {`

```
/// Adresse und Port der Gegenstelle.
```

## L387 · `pub fn covers(&self, hostname: &str) -> bool {`

```
/// Darf diese Verbindung `hostname` bedienen? Siehe `leaf_der`.
```

## L392-394 · `pub fn is_healthy(&self) -> bool {`

```
/// True while the underlying TCP connection is still usable for
/// another request — the gate for HTTP keep-alive session reuse.
/// Goes false once the peer closes or the connection errors.
```

## L410-413 · `pub mod reasons {`

```
/// Reason strings a peer can cause, named so the classifier that turns them
/// into error kinds can match the CONSTANT rather than a copy of the text.
/// Reworded literals in two places is how a message quietly becomes
/// "unknown" on the error page — which is exactly what happened to alert 40.
```

## L422-423 · `pub fn reason(&self) -> &'static str {`

```
/// Static reason string, so the cause survives the trip up through the
/// `&'static str`-typed HTTP layer to whoever asked for the page.
```

## L453 · `pub fn tls_connect(tcp_handle: usize, hostname: &str) -> Result<TlsSession, TlsError> {`

```
/// Establish a TLS 1.3 connection over an existing TCP handle.
```

## L458-460 · `pub fn tls_connect_alpn(`

```
/// As `tls_connect`, but offering an ALPN protocol list (RFC 7301) in
/// preference order, e.g. `&["h2", "http/1.1"]`. The pick is readable
/// afterwards via `TlsSession::alpn`.
```

## L466 · `let x25519_private = csprng::random_256();`

```
// Generate ephemeral key pairs for both groups
```

## L472 · `let client_hello = build_client_hello(&client_random, &x25519_public, &p384_keypair.public_uncompressed, hostname, alpn_`

```
// === ClientHello ===
```

## L476 · `let server_hello = recv_handshake_message(tcp_handle, HT_SERVER_HELLO)?;`

```
// === ServerHello ===
```

## L480 · `let mut transcript = TranscriptHash::new(cipher);`

```
// Transcript hash with correct algorithm (determined by cipher suite)
```

## L485 · `let shared_secret = match server_key_share {`

```
// === Derive handshake keys ===
```

## L501 · `let mut transcript_sh = TranscriptHash::new(cipher);`

```
// Transcript hash up to ServerHello
```

## L507 · `let client_hs_secret = ks_derive_secret(cipher, &handshake_secret, b"c hs traffic", &sh_hash);`

```
// Client/Server Handshake Traffic Secrets
```

## L511 · `let server_hs_key = ks_expand_key(cipher, &server_hs_secret);`

```
// Handshake keys
```

## L519-521 · `let mut cert_chain: Vec<Vec<u8>> = Vec::new();`

```
// === Receive encrypted handshake messages ===
// May receive ChangeCipherSpec (legacy, ignore)
// Then: EncryptedExtensions, Certificate, CertificateVerify, Finished
```

## L545 · `if plaintext.is_empty() {`

```
// Last byte of plaintext is the real content type
```

## L559 · `let mut pos = 0;`

```
// Parse handshake messages from inner data
```

## L591 · `server_finished = inner[pos + 4..hs_end].to_vec();`

```
// Do NOT add to transcript before verifying!
```

## L594 · `_ => { /* Unknown, skip */ }`

```
/* Unknown, skip */
```

## L605 · `if cert_chain.is_empty() {`

```
// === Verify certificate chain ===
```

## L611-614 · `if let Err(e) = certstore::verify_chain(&cert_refs, hostname) {`

```
// Die richtige Pruefung zuerst und unveraendert. `second_chance` kann
// nichts erlauben, was hier durchgefallen waere — sie sieht den Fehler
// erst, nachdem er feststeht, und laesst genau zwei davon nach, und auch
// die nur fuer eine vom Nutzer benannte private Adresse.
```

## L619 · `let leaf_der = cert_chain[0].clone();`

```
// Das GEPRUEFTE Blatt aufheben — nicht das, was spaeter irgendwo liegt.
```

## L622 · `let transcript_before_sf = transcript.clone();`

```
// === Verify Finished ===
```

## L632 · `let hash_len = cipher.hash_len();`

```
// === Send Client Finished ===
```

## L635 · `let mut cf_transcript = transcript_before_sf.clone();`

```
// Client Finished verify_data uses transcript including server Finished
```

## L647 · `let mut finished_msg = Vec::new();`

```
// Build the Finished handshake message
```

## L653 · `let client_nonce = build_nonce(&client_hs_iv, 0);`

```
// Encrypt and send Client Finished
```

## L661-662 · `let mut app_transcript = transcript_before_sf;`

```
// === Derive Application Keys ===
// App traffic secrets use Hash(CH..SF) — transcript including server Finished
```

## L704 · `pub fn tls_send(session: &mut TlsSession, data: &[u8]) -> Result<(), TlsError> {`

```
/// Send application data over TLS.
```

## L707 · `inner.push(CT_APPLICATION_DATA); // Inner content type`

```
// Inner content type
```

## L719 · `pub fn tls_recv(session: &mut TlsSession, buf: &mut [u8]) -> Result<usize, TlsError> {`

```
/// Receive application data over TLS.
```

## L724-726 · `pub fn tls_recv_patient(`

```
/// Wie `tls_recv`, aber der Aufrufer bestimmt, wie lange auf den Anfang einer
/// Antwort gewartet wird. Wer weiss, dass noch KEIN Byte gekommen ist, nimmt
/// `QUIET_FIRST_BYTE`; wer mitten im Koerper steht, `QUIET_TRANSFER`.
```

## L758 · `if real_ct == CT_HANDSHAKE {`

```
// Skip handshake messages (NewSessionTicket etc.) in app data phase
```

## L768-780 · `pub fn tls_poll(session: &mut TlsSession, buf: &mut [u8]) -> Result<usize, TlsError> {`

```
/// **Lauschen statt holen** — ein Satz, wenn einer vollstaendig da ist,
/// sonst `Ok(0)`, und zwar SOFORT.
///
/// `tls_recv` wartet mindestens einen Versuch (10 s Deckel, ein Tick
/// Mindestwartezeit); das ist richtig fuer eine angeforderte Antwort und
/// falsch fuer eine Verbindung, die von selbst spricht. Hier wird nur
/// abgeholt, was der TCP-Stapel schon hat, in `session.rx` gesammelt und erst
/// entschluesselt, wenn Kopf und Nutzlast beisammen sind.
///
/// `Ok(0)` heisst „noch nichts" und NICHT „zu" — ein geschlossener Strom
/// meldet sich als `Err`. Das ist dieselbe Unterscheidung, die `tls_recv`
/// schon trifft (ein `ChangeCipherSpec` oder eine Handschlagsnachricht in der
/// Datenphase sind auch 0).
```

## L782-788 · `let mut chunk = [0u8; 2048];`

```
// 1. Einsammeln, was ohne Warten da ist — aber nur, solange Platz ist.
//
// **Ein voller Puffer ist Gegendruck, kein Protokollfehler.** Wer daraus
// ein `Err` macht, beendet eine gesunde Verbindung, sobald die Gegenstelle
// einmal schneller spricht als der Rufer abholt — und genau das tut ein
// CDP-Strom. Wird hier nichts mehr abgeholt, laeuft das TCP-Fenster zu
// und die Gegenstelle hoert von selbst auf: so ist Gegendruck gedacht.
```

## L799-803 · `while session.rx_plain.len() < buf.len() {`

```
// 2. Entschluesseln, solange ganze Saetze dastehen und der Rufer noch
//    nicht genug hat. Ein uebersprungener Satz (Sitzungskarte,
//    `ChangeCipherSpec`, leerer Klartext) beendet die Runde NICHT — sonst
//    hiesse `Ok(0)` mal „noch nichts" und mal „hier lag nur nichts fuer
//    dich", und der Rufer kann die zwei nicht auseinanderhalten.
```

## L827-828 · `if real_ct == CT_HANDSHAKE { continue }`

```
// Eine Sitzungskarte mitten im Strom ist kein Datensatz — ueberspringen,
// wie `tls_recv` es tut.
```

## L833-834 · `let n = session.rx_plain.len().min(buf.len());`

```
// 3. Herausgeben, was passt. Der Rest bleibt liegen und kommt beim
//    naechsten Aufruf — **kein Byte wird weggeworfen**.
```

## L841-843 · `const RX_HIGH_WATER: usize = MAX_RECORD_PAYLOAD * 4;`

```
/// Ab hier wird nicht mehr vom TCP-Stapel nachgeladen: vier Saetze liegen
/// dann schon unentschluesselt da. Kein Deckel, der etwas verwirft — eine
/// Marke, ab der wir aufhoeren zu fragen.
```

## L847 · `pub fn tls_close(session: &mut TlsSession) -> Result<(), TlsError> {`

```
/// Close TLS session.
```

## L849 · `let mut alert = Vec::new();`

```
// Send close_notify alert
```

## L851 · `alert.push(1); // warning`

```
// warning
```

## L852 · `alert.push(0); // close_notify`

```
// close_notify
```

## L853 · `alert.push(CT_ALERT); // inner content type`

```
// inner content type
```

## L866-868 · `fn build_client_hello(random: &[u8; 32], x25519_pub: &[u8; 32], p384_pub: &[u8; 97], hostname: &str, alpn_offer: &[&str]`

```
// ============================================================
// Internal helpers
// ============================================================
```

## L873 · `let sni = build_sni_extension(hostname);`

```
// SNI extension
```

## L877 · `put_u16(&mut extensions, EXT_SUPPORTED_VERSIONS);`

```
// Supported Versions: TLS 1.3
```

## L879 · `put_u16(&mut extensions, 3); // length`

```
// length
```

## L880 · `extensions.push(2); // list length`

```
// list length
```

## L884 · `put_u16(&mut extensions, EXT_SUPPORTED_GROUPS);`

```
// Supported Groups: secp384r1 + x25519
```

## L886 · `put_u16(&mut extensions, 6); // 2 groups x 2 bytes + list_len(2)`

```
// 2 groups x 2 bytes + list_len(2)
```

## L887 · `put_u16(&mut extensions, 4); // list length`

```
// list length
```

## L891-894 · `let shares_len: u16 = 36 + 101;`

```
// Key Share: both x25519 (36 bytes) and secp384r1 (101 bytes)
// x25519 entry: group(2) + key_len(2) + key(32) = 36
// P-384 entry: group(2) + key_len(2) + key(97) = 101
// Total shares: 36 + 101 = 137
```

## L897 · `put_u16(&mut extensions, shares_len + 2); // extension data: shares_len_field(2) + shares`

```
// extension data: shares_len_field(2) + shares
```

## L898 · `put_u16(&mut extensions, shares_len);     // client_shares length`

```
// client_shares length
```

## L899 · `put_u16(&mut extensions, GROUP_SECP384R1);`

```
// secp384r1 key share (first = preferred)
```

## L903 · `put_u16(&mut extensions, GROUP_X25519);`

```
// x25519 key share
```

## L908 · `put_u16(&mut extensions, EXT_SIGNATURE_ALGORITHMS);`

```
// Signature Algorithms (offer both RSA and ECDSA for server compatibility)
```

## L910 · `put_u16(&mut extensions, 12); // extension data length`

```
// extension data length
```

## L911 · `put_u16(&mut extensions, 10); // list length`

```
// list length
```

## L912 · `put_u16(&mut extensions, 0x0403); // ecdsa_secp256r1_sha256`

```
// ecdsa_secp256r1_sha256
```

## L913 · `put_u16(&mut extensions, 0x0804); // rsa_pss_rsae_sha256 (TLS 1.3)`

```
// rsa_pss_rsae_sha256 (TLS 1.3)
```

## L914 · `put_u16(&mut extensions, 0x0401); // rsa_pkcs1_sha256`

```
// rsa_pkcs1_sha256
```

## L915 · `put_u16(&mut extensions, 0x0503); // ecdsa_secp384r1_sha384`

```
// ecdsa_secp384r1_sha384
```

## L916 · `put_u16(&mut extensions, 0x0805); // rsa_pss_rsae_sha384`

```
// rsa_pss_rsae_sha384
```

## L918-919 · `if !alpn_offer.is_empty() {`

```
// ALPN (RFC 7301) — omitted entirely when we offer nothing, which keeps
// the ClientHello byte-identical to what shipped before.
```

## L925 · `continue; // a protocol name is a 1-byte-prefixed string`

```
// a protocol name is a 1-byte-prefixed string
```

## L932 · `put_u16(&mut extensions, (list.len() + 2) as u16); // list_len field + list`

```
// list_len field + list
```

## L938 · `let mut body = Vec::new();`

```
// Build ClientHello body
```

## L940 · `body.push(TLS_VERSION_12[0]); // Legacy version`

```
// Legacy version
```

## L942 · `body.extend_from_slice(random); // 32 bytes random`

```
// 32 bytes random
```

## L944 · `let session_id = csprng::random_256();`

```
// Session ID (32 bytes random for TLS 1.3 compatibility mode)
```

## L949 · `put_u16(&mut body, 6); // 3 suites x 2 bytes`

```
// Cipher suites: all 3 TLS 1.3 suites (strongest first)
```

## L950 · `put_u16(&mut body, 6); // 3 suites x 2 bytes`

```
// 3 suites x 2 bytes
```

## L951 · `put_u16(&mut body, 0x1302); // TLS_AES_256_GCM_SHA384`

```
// TLS_AES_256_GCM_SHA384
```

## L952 · `put_u16(&mut body, 0x1301); // TLS_AES_128_GCM_SHA256`

```
// TLS_AES_128_GCM_SHA256
```

## L953 · `put_u16(&mut body, 0x1303); // TLS_CHACHA20_POLY1305_SHA256`

```
// TLS_CHACHA20_POLY1305_SHA256
```

## L955 · `body.push(1); // Length`

```
// Compression methods
```

## L956 · `body.push(1); // Length`

```
// Length
```

## L957 · `body.push(0); // null compression`

```
// null compression
```

## L959 · `put_u16(&mut body, extensions.len() as u16);`

```
// Extensions
```

## L963 · `let mut msg = Vec::new();`

```
// Wrap in handshake header
```

## L971-974 · `fn parse_alpn_extension(data: &[u8]) -> Option<String> {`

```
/// Pull the server's ALPN pick out of an EncryptedExtensions body (RFC 7301
/// §3.2). `data` starts at the 2-byte extension-list length. The server sends
/// exactly one protocol name; anything malformed yields `None`, which the
/// caller reads as "no ALPN" and thus HTTP/1.1.
```

## L990 · `let name_len = data[body + 2] as usize;`

```
// ProtocolNameList: u16 list length, then u8-prefixed names.
```

## L1009 · `put_u16(&mut ext, (name.len() + 5) as u16); // extension data length`

```
// extension data length
```

## L1010 · `put_u16(&mut ext, (name.len() + 3) as u16); // server name list length`

```
// server name list length
```

## L1011 · `ext.push(0); // host_name type`

```
// host_name type
```

## L1019 · `let mut pos = 4; // Skip handshake header`

```
// Skip handshake header
```

## L1022 · `pos += 2; // version`

```
// version
```

## L1025 · `pos += 32; // server random`

```
// server random
```

## L1031 · `if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no cipher suite")); }`

```
// Cipher suite selected by server
```

## L1041 · `pos += 1; // compression`

```
// compression
```

## L1043 · `if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no extensions")); }`

```
// Extensions
```

## L1057 · `let group = ((msg[pos] as u16) << 8) | msg[pos + 1] as u16;`

```
// group(2) + key_len(2) + key(N)
```

## L1093 · `if pos >= data.len() { return certs; }`

```
// Request context (1 byte length + context)
```

## L1098 · `if pos + 3 > data.len() { return certs; }`

```
// Certificate list (3-byte length)
```

## L1111 · `if pos + 2 <= list_end {`

```
// Skip extensions (2-byte length + data)
```

## L1135-1136 · `let mut header = [0u8; 5];`

```
// Read 5-byte header. Die Geduld gilt dem KOPF — sobald er da ist, ist die
// Gegenstelle am Antworten und die Nutzlast bekommt die volle Nachsicht.
```

## L1153-1157 · `pub const QUIET_TRANSFER: u32 = 6;`

```
/// Patience for a record that is already flowing: a quiet link is not a
/// closed one. Each attempt waits 10 s; six of them give a minute, the same
/// order as the TCP retransmit budget (TCP_RETR2). Under a run of transmit
/// stalls a single attempt ended the record — and the caller saw a truncated
/// body it could only report as "short download".
```

## L1160 · `pub const ATTEMPT_TICKS: u64 = 1000; // 10 s`

```
/// Wie lange ein Versuch wartet, in Ticks (100 Hz).
```

## L1161 · `pub const ATTEMPT_TICKS: u64 = 1000; // 10 s`

```
// 10 s
```

## L1163-1164 · `pub const QUIET_FIRST_BYTE: u32 = 1;`

```
/// Patience for the FIRST byte of an answer, auf einer FRISCHEN Verbindung.
/// Ein Server darf sich Zeit lassen, bevor er zu antworten beginnt.
```

## L1167-1177 · `pub const ATTEMPT_TICKS_REUSED: u64 = 100; // 1 s`

```
/// Und auf einer WIEDERVERWENDETEN. Hier ist die Wette eine andere: der
/// Server kann zwischen zwei Benutzungen weggegangen sein, ohne FIN und ohne
/// RST — gemessen an thumb.wikimedia.org, das nach jeder bedienten Runde
/// still nicht mehr antwortet. Vorhersagen laesst sich das nicht (nach
/// `poll_rx_only` sah die Verbindung gesund aus), also zaehlt nur, wie
/// schnell es auffaellt.
///
/// Die Zahl ist am PREIS DER ALTERNATIVE bemessen, nicht geraten: ein
/// frischer Aufbau zu diesem Host kostet gemessen 50-80 ms (`tcp 20 + tls 30
/// + preface 0`). Laenger als gut zehnmal so lange zu warten, nur um ihn zu
/// sparen, ist ein schlechtes Geschaeft.
```

## L1178 · `pub const ATTEMPT_TICKS_REUSED: u64 = 100; // 1 s`

```
// 1 s
```

## L1223 · `for i in 0..8 {`

```
// XOR sequence number into the last 8 bytes of IV
```

