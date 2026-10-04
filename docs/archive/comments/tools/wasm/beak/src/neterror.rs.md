# `tools/wasm/beak/src/neterror.rs` @ 5e0102684

## L1-13 · `use alloc::string::String;`

```
//! The page shown when a fetch fails.
//!
//! It is an ordinary HTML document handed to the engine, the same way
//! Firefox's `about:neterror` is — so it costs no rendering code and
//! inherits the reader's theme. Deliberately self-contained: inline
//! `<style>` only, no links, no images, nothing to fetch. A failure page
//! that needs the network is a failure page that shows a blank screen.
//!
//! No "continue anyway" button. A click-through is a capability grant, and
//! one bolted onto the panic moment is how every browser taught people to
//! dismiss certificate warnings without reading them. If it is ever added
//! it belongs pinned to one host and one fingerprint, visible and
//! revocable afterwards — not here.
```

## L17-22 · `fn wording(kind: &str) -> (&'static str, &'static str, &'static [&'static str]) {`

```
/// Headline, explanation, and any concrete next steps for a failure kind.
///
/// These are HTML fragments, not plain text — they may carry `<code>`, and
/// so must spell literal angle brackets as entities. They are ours, fixed
/// at compile time; only the URL and the reason from the network layer are
/// escaped, because only those come from outside.
```

## L123-124 · `pub fn document(url: &str, kind: &str, message: &str) -> String {`

```
/// Build the document. `url` is what was asked for, `kind`/`message` come
/// from `npk_http_last_error`.
```

## L129-131 · `s.push_str(`

```
// No colours anywhere: the engine paints text, headings, muted text and
// rules from the active theme, so leaving them unset is what makes this
// page follow light/dark instead of fighting it.
```

## L151-155 · `let device_hint = matches!(kind, "cert.hostname" | "cert.untrusted")`

```
// **Der Geraetefall bekommt seinen eigenen Hinweis, mit der Adresse
// darin.** Ein Router im Heimnetz KANN kein oeffentlich vertrautes
// Zertifikat haben — keine CA stellt eines fuer eine private Adresse
// aus. Ohne diesen Absatz endet der Weg hier, und der Nutzer haelt es
// fuer einen Fehler in beak. Mit ihm steht der naechste Schritt da.
```

## L175-177 · `s.push_str("<hr><p class=\"detail\">");`

```
// The address and the raw reason go last and verbatim. This is the part
// worth reporting or searching for, and paraphrasing it would cost the
// one detail that identifies the actual failure.
```

## L186-191 · `fn private_host_of(url: &str) -> Option<String> {`

```
/// Die Adresse aus einer URL, WENN sie eine literale private ist.
///
/// Nur dann ist der Geraetehinweis wahr — und nur dann nimmt der Kernel den
/// Schalter ueberhaupt an (`net.lan_devices` gilt ausschliesslich fuer
/// literale private Adressen). Ein Hinweis, der auf einen Weg zeigt, den
/// der Kernel gleich wieder verwirft, waere schlimmer als keiner.
```

## L209-211 · `fn escape_into(text: &str, out: &mut String) {`

```
/// Escape text for HTML. The URL is attacker-influenced — it can come from a
/// redirect target — so it must never be able to close a tag and inject
/// markup into our own error page.
```

