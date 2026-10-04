//! The built-in test page: `beak:selftest`.
//!
//! Unlike external sites it answers the same way every time, fetches
//! nothing, and reports its result twice: on screen and in the log.
//!
//! The scaffold is deliberately old-fashioned (`var`, `function`); every
//! modern construct is checked individually through `Function()`, so one
//! unsupported feature produces one failing line instead of a blank page.

/// The document. No link, no image, no external script.
///
/// Kept as a separate file so the same text also runs through the engine
/// on the host (`beak-engine/examples/selftest.rs`).
pub const HTML: &str = include_str!("selftest.html");

/// The address the page lives at.
pub const URL: &str = "beak:selftest";

/// Is this the test page? `about:` is accepted too, because other browsers
/// keep such pages there and the typo would otherwise end as a web search.
pub fn matches(url: &str) -> bool {
    let u = url.trim();
    u.eq_ignore_ascii_case(URL)
        || u.eq_ignore_ascii_case("about:selftest")
        || u.eq_ignore_ascii_case("beak:test")
}
