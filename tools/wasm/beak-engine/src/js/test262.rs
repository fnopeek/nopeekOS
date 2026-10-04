//! `$262`, the host object of the test262 runner, and only there.
//!
//! test262 expects every host to provide `$262` with hooks the language
//! cannot offer itself: detach a buffer, evaluate a script in global scope,
//! call the collector. It is exposed only when the host enables it, like
//! `crypto` (`super::random`): a page must never see it, since `evalScript`
//! would bypass script delivery and `detachArrayBuffer` could pull buffers
//! out from under live views.
//!
//! Not implemented:
//!
//! * `createRealm`: a second realm is a second `Interp`, with its own
//!   Rc cycles to break on drop.
//! * `IsHTMLDDA`: the `[[IsHTMLDDA]]` exotic (`document.all`), an object
//!   that behaves like `undefined` under ToBoolean and `typeof`; needs an
//!   object model change.
//! * `agent`: Atomics/SharedArrayBuffer are skipped (`SKIP_FEATURES_EXEC`),
//!   so a hook for them would promise something that does not run.

/// Whether the host enabled `$262`. Default: no.
static mut ON: bool = false;

/// Called once by the runner at startup.
pub fn enable() {
    // SAFETY: single-threaded engine; written once before any reader runs.
    unsafe { core::ptr::addr_of_mut!(ON).write(true) };
}

/// Whether `$262` is on the global object.
pub fn enabled() -> bool {
    // SAFETY: single-threaded engine; plain read of a `Copy` static.
    unsafe { core::ptr::addr_of!(ON).read() }
}
