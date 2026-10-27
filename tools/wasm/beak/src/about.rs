//! The built-in page `beak:about`: name, version and licence, from the
//! binary like `beak:selftest`.

pub const URL: &str = "beak:about";

pub const HTML: &str = concat!(
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>About beak</title></head>",
    "<body style=\"font-family:sans-serif;max-width:40em;margin:3em auto\">",
    "<h1>beak</h1><p>Version ", env!("CARGO_PKG_VERSION"), "</p>",
    "<p>The native web browser of nopeekOS: its own HTML, CSS and JavaScript ",
    "engine, running as a sandboxed WASM module.</p>",
    "<p>Licence: ", env!("CARGO_PKG_LICENSE"), ".</p>",
    "</body></html>");

/// Is this the about page? `about:beak` is accepted too.
pub fn matches(url: &str) -> bool {
    let u = url.trim();
    u.eq_ignore_ascii_case(URL) || u.eq_ignore_ascii_case("about:beak")
}
