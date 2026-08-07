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

use alloc::string::String;

/// Headline, explanation, and any concrete next steps for a failure kind.
///
/// These are HTML fragments, not plain text — they may carry `<code>`, and
/// so must spell literal angle brackets as entities. They are ours, fixed
/// at compile time; only the URL and the reason from the network layer are
/// escaped, because only those come from outside.
fn wording(kind: &str) -> (&'static str, &'static str, &'static [&'static str]) {
    match kind {
        "cert.untrusted" => (
            "This site's identity can't be verified",
            "The certificate is signed by an authority this system does not trust. \
             That is expected for a private or self-signed certificate, and it is \
             also what an intercepted connection looks like.",
            &[
                "If you run this site yourself, trust its CA with <code>cert add &lt;file&gt;</code>.",
                "Otherwise: on a network you don't control, treat this as interception.",
                "<code>cert list</code> shows every authority this system currently trusts.",
            ],
        ),
        "cert.expired" => (
            "This site's certificate has expired",
            "The certificate is past its validity period. Usually the site let it \
             lapse — but a system clock set to the wrong date produces exactly the \
             same result.",
            &["Check this system's date before assuming the site is at fault."],
        ),
        "cert.not_yet_valid" => (
            "This site's certificate isn't valid yet",
            "The certificate's validity period begins in the future. Almost always \
             this system's clock is behind, not the site being early.",
            &["Check this system's date."],
        ),
        "cert.hostname" => (
            "This certificate belongs to a different site",
            "The certificate is valid, but it was issued for another domain. A \
             server misconfiguration looks like this — so does someone presenting \
             a certificate they do hold for a site they don't.",
            &[],
        ),
        "cert.invalid" => (
            "This site's certificate can't be accepted",
            "The certificate chain is malformed or violates a rule it has to \
             satisfy. The detail below names which one.",
            &[],
        ),
        "dns.failed" => (
            "This address couldn't be looked up",
            "No DNS answer came back for this host name. Either the name does not \
             exist, or name resolution isn't reaching anything right now.",
            &["Check the spelling of the address.", "Check the network connection."],
        ),
        "tls.handshake" | "tls.protocol" => (
            "This site refused to negotiate a secure connection",
            "The server rejected the connection before any certificate was \
             exchanged — so this is not a problem with its identity. Usually \
             it wants an option this browser does not offer, or it is \
             misconfigured.",
            &[],
        ),
        "net.connect" => (
            "This site didn't accept the connection",
            "The address resolved, but nothing answered on it. The server may be \
             down, or something between here and it is refusing the connection.",
            &[],
        ),
        "net.timeout" => (
            "This site took too long to answer",
            "The connection was established but the response never arrived within \
             the time allowed.",
            &[],
        ),
        "net.reset" => (
            "The connection was cut",
            "The connection closed part-way through the response.",
            &[],
        ),
        "http.status" => (
            "This site returned an error",
            "The server answered, but with an error status rather than a page.",
            &[],
        ),
        "http.empty" => (
            "This site returned an empty page",
            "The request succeeded but the response carried no content.",
            &[],
        ),
        "http.redirect" => (
            "This address redirected somewhere it shouldn't",
            "The site sent a redirect that could not be followed — missing, \
             malformed, looping, or stepping down from HTTPS to plain HTTP.",
            &[],
        ),
        "url.invalid" => (
            "That isn't a usable address",
            "The address could not be parsed as a URL.",
            &[],
        ),
        _ => (
            "This page couldn't be loaded",
            "The request did not complete. The detail below is what the network \
             layer reported.",
            &[],
        ),
    }
}

/// Build the document. `url` is what was asked for, `kind`/`message` come
/// from `npk_http_last_error`.
pub fn document(url: &str, kind: &str, message: &str) -> String {
    let (title, body, steps) = wording(kind);

    let mut s = String::with_capacity(2048);
    // No colours anywhere: the engine paints text, headings, muted text and
    // rules from the active theme, so leaving them unset is what makes this
    // page follow light/dark instead of fighting it.
    s.push_str(
        "<style>\
         .wrap{max-width:38em;margin:3.5em auto;padding:0 2em}\
         h1{font-size:1.5em;line-height:1.3;margin:0 0 0.7em 0}\
         p{margin:0 0 1em 0;line-height:1.55}\
         ul{margin:0 0 1em 1.4em;padding:0}\
         li{margin:0 0 0.45em 0;line-height:1.5}\
         hr{border:0;border-top:1px solid;margin:2.2em 0 1.2em 0}\
         .detail{font-family:monospace;font-size:0.88em;line-height:1.7}\
         code{font-family:monospace}\
         </style>",
    );

    s.push_str("<div class=\"wrap\"><h1>");
    s.push_str(title);
    s.push_str("</h1><p>");
    s.push_str(body);
    s.push_str("</p>");

    if !steps.is_empty() {
        s.push_str("<ul>");
        for step in steps {
            s.push_str("<li>");
            s.push_str(step);
            s.push_str("</li>");
        }
        s.push_str("</ul>");
    }

    // The address and the raw reason go last and verbatim. This is the part
    // worth reporting or searching for, and paraphrasing it would cost the
    // one detail that identifies the actual failure.
    s.push_str("<hr><p class=\"detail\">");
    escape_into(url, &mut s);
    s.push_str("<br>");
    escape_into(message, &mut s);
    s.push_str("</p></div>");
    s
}

/// Escape text for HTML. The URL is attacker-influenced — it can come from a
/// redirect target — so it must never be able to close a tag and inject
/// markup into our own error page.
fn escape_into(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}
