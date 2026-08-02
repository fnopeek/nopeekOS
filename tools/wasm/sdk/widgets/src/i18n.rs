//! UI language selection.
//!
//! The kernel stores a language code only (`lang` config key, read via
//! `npk_locale`); the strings live in the app. An app declares a plain
//! struct of `&'static str` fields and one `const` per language — the
//! compiler then refuses any catalog that forgot a field, so a new
//! string can never silently fall back to English.
//!
//! ```ignore
//! struct Strings { file: &'static str, edit: &'static str }
//! const EN: Strings = Strings { file: "File", edit: "Edit" };
//! const DE: Strings = Strings { file: "Datei", edit: "Bearbeiten" };
//!
//! fn s() -> &'static Strings {
//!     match i18n::lang() { Lang::De => &DE, _ => &EN }
//! }
//! ```
//!
//! English is the source language and the fallback for any unknown code.

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_locale(buf_ptr: i32, buf_max: i32) -> i32;
}

/// Languages the UI ships catalogs for. Adding one is an SDK bump plus a
/// `const` per app — the kernel is not involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
}

impl Lang {
    /// Map an ISO-639-1 code (or a `de_CH`-style tag) to a catalog.
    /// Unknown codes fall back to English.
    pub fn from_code(code: &str) -> Lang {
        let primary = code.trim().split(['-', '_']).next().unwrap_or("");
        if primary.eq_ignore_ascii_case("de") {
            Lang::De
        } else {
            Lang::En
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::De => "de",
        }
    }
}

// Resolved once per app run. The language cannot change under a running
// app — a `set lang` takes effect on the next launch — so caching is
// safe and keeps `lang()` free to call inside a render loop.
static mut CACHED: Option<Lang> = None;

/// The active UI language.
pub fn lang() -> Lang {
    // SAFETY: WASM apps are single-threaded; no other execution context
    // can observe or mutate `CACHED` between the read and the write.
    unsafe {
        if let Some(l) = *(&raw const CACHED) {
            return l;
        }
        let resolved = query();
        *(&raw mut CACHED) = Some(resolved);
        resolved
    }
}

#[cfg(target_arch = "wasm32")]
fn query() -> Lang {
    let mut buf = [0u8; 16];
    let n = unsafe { npk_locale(buf.as_mut_ptr() as i32, buf.len() as i32) };
    if n <= 0 {
        return Lang::En;
    }
    match core::str::from_utf8(&buf[..n as usize]) {
        Ok(code) => Lang::from_code(code),
        Err(_) => Lang::En,
    }
}

/// Host builds (SDK unit tests) have no kernel to ask.
#[cfg(not(target_arch = "wasm32"))]
fn query() -> Lang {
    Lang::En
}
