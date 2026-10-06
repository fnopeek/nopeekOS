//! The host bridge, from one list for both engines.
//!
//! Each entry names a `host_core` function and its wasm signature. From
//! that the macro emits the forge adapter, the import resolver with the
//! signature forge checks against the module's declaration, and the wasmi
//! registration. `ctx` entries get the instance state, `mem` entries guest
//! memory as well.
//!
//! Calling convention set by the code generator: `rdi` = vmctx, integer
//! arguments from `rsi`, return in `rax`. That is SysV, so `extern "C"` fits
//! as is, including functions with more than five arguments, which spill to
//! the stack.

use super::host_core;
use super::HostState;
use forge_core::vmctx;

/// Runs `f` on the instance state. The reference cannot outlive the call.
pub(crate) fn with_ctx<R>(vm: *const u64, f: impl FnOnce(&mut HostState) -> R) -> R {
    // SAFETY: `vm` is the vmctx generated code passes in `rdi`; `HOST_CTX`
    // was set by `NpkHost` and points at a `HostState` that outlives the
    // instance. No host function re-enters the guest, so nothing else holds
    // a reference to it during the call.
    f(unsafe { &mut *(*vm.add(vmctx::HOST_CTX as usize / 8) as *mut HostState) })
}

/// Runs `f` on guest memory and the instance state, re-read on every call:
/// the base never moves, but `memory.grow` moves the end.
pub(crate) fn with_parts<R>(vm: *const u64, f: impl FnOnce(&mut [u8], &mut HostState) -> R) -> R {
    // SAFETY: as in `with_ctx`; `MEM_BASE`/`MEM_SIZE` describe this
    // instance's mapped memory, which is disjoint from the `HostState`.
    let (base, size) = unsafe {
        (*vm.add(vmctx::MEM_BASE as usize / 8), *vm.add(vmctx::MEM_SIZE as usize / 8) as usize)
    };
    // A module without memory gets an empty slice; the host functions report
    // out-of-bounds for it.
    let mem: &mut [u8] = if base == 0 {
        &mut []
    } else {
        // SAFETY: see above; the slice lives only for this call.
        unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size) }
    };
    with_ctx(vm, |ctx| f(mem, ctx))
}

/// What a wasmi `mem` call returns when the module exports no memory.
pub(crate) trait NoMemory {
    fn no_memory() -> Self;
}
impl NoMemory for i32 { fn no_memory() -> Self { -1 } }
impl NoMemory for i64 { fn no_memory() -> Self { -1 } }
impl NoMemory for () { fn no_memory() -> Self {} }

/// Emits, for one namespace: the forge adapters, `resolve_table` and
/// `register_wasmi`. `$host` is the module holding the implementations.
macro_rules! host_imports {
    (
        ns: $ns:literal, host: $host:ident, no_memory: $nm:expr;
        $( $kind:ident fn $name:ident ( $( $a:ident : $t:ident ),* ) $( -> $r:ident )? ; )*
    ) => {
        mod adapters {
            #![allow(clippy::too_many_arguments)]
            use super::*;
            $(
                pub(super) extern "C" fn $name(vm: *const u64 $(, $a: $t)*) $(-> $r)? {
                    $crate::wasm::forge_glue::host_imports!(@forge $kind $host vm $name ($($a),*))
                }
            )*
        }

        /// Address and signature of the adapter for an import, or `None`.
        pub(crate) fn resolve_table(module: &str, name: &str)
            -> Option<(u64, $crate::forge_rt::HostSig)>
        {
            if module != $ns {
                return None;
            }
            match name {
                $( stringify!($name) => Some((
                    adapters::$name as *const () as u64,
                    $crate::forge_rt::HostSig {
                        params: &[$($crate::wasm::forge_glue::host_imports!(@wt $t)),*],
                        result: $crate::wasm::forge_glue::host_imports!(@ret $($r)?),
                    },
                )), )*
                _ => None,
            }
        }

        /// The same functions for the interpreter.
        pub(crate) fn register_wasmi(
            linker: &mut wasmi::Linker<$crate::wasm::HostState>,
        ) -> Result<(), wasmi::Error> {
            $(
                linker.func_wrap($ns, stringify!($name),
                    |mut caller: wasmi::Caller<'_, $crate::wasm::HostState> $(, $a: $t)*| $(-> $r)? {
                        $crate::wasm::forge_glue::host_imports!(@wasmi $kind $host caller ($nm) $name ($($a),*))
                    })?;
            )*
            Ok(())
        }
    };

    (@forge ctx $h:ident $vm:ident $n:ident ($($a:ident),*)) => {
        $crate::wasm::forge_glue::with_ctx($vm, |c| $h::$n(c $(, $a)*))
    };
    (@forge mem $h:ident $vm:ident $n:ident ($($a:ident),*)) => {
        $crate::wasm::forge_glue::with_parts($vm, |m, c| $h::$n(m, c $(, $a)*))
    };
    (@wasmi ctx $h:ident $c:ident ($nm:expr) $n:ident ($($a:ident),*)) => {
        $h::$n($c.data_mut() $(, $a)*)
    };
    (@wasmi mem $h:ident $c:ident ($nm:expr) $n:ident ($($a:ident),*)) => {{
        let Some(m) = $c.get_export("memory").and_then(|e| e.into_memory()) else {
            return $nm;
        };
        let (mem, st) = m.data_and_store_mut(&mut $c);
        $h::$n(mem, st $(, $a)*)
    }};
    (@wt i32) => { $crate::forge_rt::Wt::I32 };
    (@wt i64) => { $crate::forge_rt::Wt::I64 };
    (@ret) => { None };
    (@ret $r:ident) => { Some($crate::wasm::forge_glue::host_imports!(@wt $r)) };
}
pub(crate) use host_imports;

/// What `forge_rt` queries to fill the import table.
pub(crate) struct NpkHost(pub(crate) *mut HostState);

impl crate::forge_rt::HostImports for NpkHost {
    fn ctx_ptr(&self) -> u64 {
        self.0 as u64
    }
    fn resolve(&self, module: &str, name: &str) -> Option<(u64, crate::forge_rt::HostSig)> {
        resolve(module, name)
    }
}

/// Any import of either ABI (`env` or wasi): a module may use both, so the
/// host answers both instead of making the caller pick one.
pub(crate) fn resolve(module: &str, name: &str) -> Option<(u64, crate::forge_rt::HostSig)> {
    resolve_table(module, name).or_else(|| crate::wasi::forge_glue::resolve(module, name))
}

host_imports! {
    ns: "env", host: host_core, no_memory: crate::wasm::forge_glue::NoMemory::no_memory();

    // npk_print(ptr, len) — write to output buffer or directly to terminal.
    // Routed through one place so npk_print and wasi fd_write cannot drift.
    mem fn npk_print(ptr: i32, len: i32);

    // npk_log(ptr, len) — write to serial console (no cap needed, output only)
    mem fn npk_log(ptr: i32, len: i32);

    // npk_log_serial(ptr, len) — write directly to the serial port,
    // bypassing the shade-terminal write path used by kprintln.
    //
    // Needed by widget-only apps (drun) that run when no terminal
    // window exists: kprintln locks SERIAL and routes a copy through
    // `shade::terminal::write`, which can stall during early boot or
    // when the active-terminal slot has no backing buffer. Direct
    // serial lives inside the same SERIAL mutex but skips the
    // terminal-side work, so it is safe to call from a worker core
    // regardless of shade state.
    mem fn npk_log_serial(ptr: i32, len: i32);

    // npk_fetch(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
    mem fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_request(url_ptr, url_len, buf_ptr, buf_max) -> bytes or -1
    // Outbound HTTPS GET for the native browser. Parses the URL,
    // fetches the body (following redirects) via the same TLS path OTA
    // uses, and copies up to buf_max bytes into the caller's buffer.
    // NET-gated — distinct from npkFS READ and from WiFi NETCTL.
    mem fn npk_http_request(url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_send(method_ptr, method_len, url_ptr, url_len,
    //               hdrs_ptr, hdrs_len, body_ptr, body_len,
    //               buf_ptr, buf_max) -> bytes, or -1
    //
    // The general form of `npk_http_request`: any method, caller-supplied
    // headers, a request body, and the response's status + headers readable
    // afterwards (needed for POST logins and `Set-Cookie`).
    //
    // `hdrs` is newline-separated `Name: value` lines. Cookie policy stays
    // out of the kernel — which cookie belongs on which request is RFC 6265,
    // and that is the browser's job; the kernel only carries bytes.
    //
    // A non-2xx does not fail here: a 404 page and a 403 explaining itself
    // are documents a person needs to read. The status comes back through
    // `npk_http_status`.
    //
    // NET-gated, same capability as npk_http_request.
    mem fn npk_http_send(method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_response_headers(buf_ptr, buf_max) -> len, or -1
    // The last npk_http_send response's header block, minus the status line.
    // `Set-Cookie` repeats, so this is a raw block rather than a getter per
    // name. NET-gated.
    mem fn npk_http_response_headers(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_status() -> status, or 0
    // The last npk_http_send response's HTTP status. NET-gated.
    ctx fn npk_http_status() -> i32;

    // npk_http_request_many(urls_ptr, urls_len, out_ptr, out_max,
    //                       lens_ptr, lens_max) -> count, or -1
    //
    // Fetch many URLs in one call, multiplexed over HTTP/2 where the host
    // offers it. `urls` is a newline-separated list; the bodies are written
    // back-to-back into `out`, and `lens` receives one little-endian i32 per
    // URL — the byte count written, or -1 for a resource that failed or did
    // not fit. The guest walks `lens` to slice `out`.
    //
    // Exists because sequential HTTP/1.1 sub-resource fetching is what walks
    // a page into rate limits and spends a round-trip per file. NET-gated,
    // same capability as npk_http_request.
    mem fn npk_http_request_many(urls_ptr: i32, urls_len: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32) -> i32;

    // ── Fetching without standing still ────────────────────────────────
    //
    // The two requests above, split into "start it" and "collect it". A
    // module that calls npk_http_send is inside the host call until the
    // exchange ends — it cannot paint, cannot read a key, and its peer
    // fibers do not run. These five let it keep its loop: begin -> handle,
    // poll between frames, take when the answer is there. The wait itself
    // happens on a worker fiber on another core (intent::fetch).
    //
    // NET-gated, same capability as the synchronous pair.

    // npk_net_context(url_ptr, url_len) -> 0, or -1
    //
    // The browser declares which document it is showing. The kernel resolves
    // the address itself and keeps only the class (public / private / local),
    // never the name, since a name may resolve elsewhere the second time.
    //
    // Without this call the module counts as a public page; that default is
    // the strict one, so forgetting the call is safe.
    mem fn npk_net_context(url_ptr: i32, url_len: i32) -> i32;

    // npk_http_begin(method_ptr, method_len, url_ptr, url_len,
    //                hdrs_ptr, hdrs_len, body_ptr, body_len, buf_max)
    //   -> handle >= 1, or -1 (reason via npk_http_last_error)
    mem fn npk_http_begin(method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_max: i32) -> i32;

    // npk_http_begin_many(urls_ptr, urls_len, out_max) -> handle >= 1, or -1
    mem fn npk_http_begin_many(urls_ptr: i32, urls_len: i32, out_max: i32) -> i32;

    // npk_http_begin_many_hdr(urls_ptr, urls_len, hdrs_ptr, hdrs_len, out_max)
    // As above, but with a cookie line per URL. A separate function rather
    // than a changed signature, which would break linking of existing modules.
    mem fn npk_http_begin_many_hdr(urls_ptr: i32, urls_len: i32, hdrs_ptr: i32, hdrs_len: i32, out_max: i32) -> i32;

    // npk_http_poll(handle) -> 1 answer waiting, 0 running, -1 failed,
    //                          -2 no such handle
    ctx fn npk_http_poll(handle: i32) -> i32;

    // npk_http_take(handle, buf_ptr, buf_max) -> bytes, -1 failed,
    //                                            -2 unknown, -3 still running
    // Frees the job and fills the same five getters npk_http_send fills.
    mem fn npk_http_take(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_take_many(handle, out_ptr, out_max, lens_ptr, lens_max)
    //   -> count, -1 failed, -2 unknown, -3 still running
    mem fn npk_http_take_many(handle: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32) -> i32;

    // npk_http_cancel(handle) -> 0. Idempotent.
    ctx fn npk_http_cancel(handle: i32) -> i32;

    // npk_random_bytes(buf_ptr, len) -> bytes written, or -1
    //
    // Randomness from the kernel CSPRNG (ChaCha20 seeded from RDRAND), backing
    // `crypto.getRandomValues`. No capability, like `npk_unix_time`: it reads
    // nothing. Capped at 64 KiB per call (WebCrypto 10.1.1) so a module cannot
    // hold the RNG mutex arbitrarily long.
    mem fn npk_random_bytes(buf_ptr: i32, len: i32) -> i32;

    // npk_http_final_url(buf_ptr, buf_max) -> len, or -1
    // The URL the last npk_http_request's body actually came from, after
    // redirects. A browser resolves relative sub-resources against this
    // (the document base URL) — resolving against the requested URL
    // instead makes every sub-resource repeat the redirect. NET-gated.
    mem fn npk_http_final_url(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_content_type(buf_ptr, buf_max) -> len, or -1
    //
    // The last npk_http_request's Content-Type, verbatim (e.g.
    // "text/html; charset=ISO-8859-1"). Cleared when the request failed.
    //
    // A document's bytes do not say what encoding they are in. Without this
    // a browser can only assume UTF-8, and one byte that is not valid UTF-8
    // costs it the entire page. NET-gated, like the request it describes.
    mem fn npk_http_content_type(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_http_last_error(buf_ptr, buf_max) -> len, or -1
    //
    // Why the last npk_http_request failed: `kind\tmessage`, where kind is
    // a stable token (`cert.untrusted`, `cert.expired`, `net.connect`, …)
    // and message is the human wording. Cleared on success.
    //
    // The request itself only returns -1, so without this a browser cannot
    // tell a rejected certificate from an empty document. NET-gated, like the
    // request whose outcome it describes.
    mem fn npk_http_last_error(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_store(name_ptr, name_len, data_ptr, data_len) -> 0 or -1
    mem fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;

    // npk_home_dir(buf_ptr, buf_max) -> i32
    // Write the current user's home directory ("home/<name>", or "home"
    // if unset) into the caller's buffer; returns bytes written or -1.
    // Apps need this because the username lives in the single encrypted
    // `.system/config` blob, not a fetchable `sys/config/name` object —
    // so they can't derive their home/documents path on their own.
    // READ-gated (it reveals the user identity from config).
    mem fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_fs_usage() -> i64
    // Filesystem fill level as (used_mib << 32) | total_mib, or -1 when
    // nothing is mounted. Feeds the file browser's capacity meter.
    // READ-gated — it says how much of the disk is in use.
    ctx fn npk_fs_usage() -> i64;

    // npk_locale(buf_ptr, buf_max) -> i32
    // Write the UI language code (`lang` config key, e.g. "en" / "de")
    // into the caller's buffer; returns bytes written or -1. Defaults to
    // "en". The kernel stores the code only — the catalogs live in the
    // apps, so adding a language never touches the kernel.
    //
    // Deliberately ungated: which language to draw labels in is a display
    // preference, not access to data. Gating it on READ would force a
    // render-only app to take a filesystem capability just to spell its
    // own menu.
    mem fn npk_locale(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_launch_arg(buf_ptr, buf_max) -> i32
    // Read the launch argument the app was started with (e.g. a file
    // path passed by npk_open). Returns bytes written, 0 if none, -1 on
    // error. Apps call this once at startup.
    mem fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;

    // ── Clipboard (cross-app copy/paste) ──────────────────────────────
    //
    // A single kernel-owned selection buffer (crate::shade::clipboard).
    // Gated on RENDER + focus: only the currently focused widget app may
    // read or write it — a background app cannot snoop the clipboard, the
    // same focus-ambient contract as receiving keystrokes. A dedicated
    // CLIPBOARD cap would need a second `.npk.caps` byte; the 1-byte
    // section is full.

    // npk_clipboard_set(ptr, len) -> i32
    // Copy `len` UTF-8 bytes from guest memory into the clipboard as Text.
    // Returns bytes stored, or -1 (denied / not focused / bad ptr).
    mem fn npk_clipboard_set(ptr: i32, len: i32) -> i32;

    // npk_clipboard_len() -> i32
    // Byte length of the current clipboard text (0 if empty). Lets an app
    // size its buffer before npk_clipboard_get. Focus-gated like the rest.
    ctx fn npk_clipboard_len() -> i32;

    // npk_clipboard_get(ptr, max) -> i32
    // Write up to `max` clipboard bytes into the guest buffer. Returns the
    // full text length (so the app can detect truncation and re-query with
    // a bigger buffer), 0 if empty, or -1 (denied / not focused / bad ptr).
    mem fn npk_clipboard_get(ptr: i32, max: i32) -> i32;

    // npk_open(app_ptr, app_len, arg_ptr, arg_len) -> i32
    // Launch widget module `app` (sys/wasm/<app>) with `arg` as its launch
    // argument (read by the app via npk_launch_arg). The launched app gets
    // its own per-app caps (from its `.npk.caps` section) and a fresh
    // window. EXECUTE-gated (launch authority). Used by loft for file
    // associations — open a file with its handler app. The kernel stays
    // generic: the ext→app mapping lives in the caller (loft + config),
    // never here.
    mem fn npk_open(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;

    // npk_launch(app_ptr, app_len, arg_ptr, arg_len) -> 0 / -1
    // Fire-and-forget launch of sys/wasm/<app> with `arg` as its launch
    // argument + per-app caps — like npk_open but without a pre-created
    // window and without singleton routing. The window (if any) is created
    // lazily on the app's first scene_commit, so a one-shot tool that
    // never commits (e.g. a full-screen screenshot) never shows a window
    // — and so never appears in its own capture. EXECUTE-gated.
    mem fn npk_launch(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;

    // npk_pick(mode, start_ptr, start_len, suggest_ptr, suggest_len, tag) -> i32
    // Open a file dialog and get the answer back as `Event::Picked`.
    //
    //   mode 0 = open an existing file, 1 = choose a save target
    //   start   = directory to open in ("" → the user's home)
    //   suggest = pre-filled filename, save mode only
    //   tag     = returned unchanged in the event (the roundtrip is async,
    //             so an app with several dialogs tells them apart by it)
    //
    // The picker module is named by `sys/config/picker` (default `pick`),
    // never by the caller: the whole point is that the dialog is a piece
    // of trusted UI the requester cannot substitute. So this is RENDER-
    // gated, not EXECUTE-gated — asking for a dialog must not require the
    // right to launch arbitrary modules, or an app would need more
    // authority to pick a file than to write one.
    //
    // The requester needs no READ to browse: the picker does the listing
    // in its own sandbox and hands back a single path.
    //
    //   0  → dialog opened
    //   -1 → cap denied / no window / bad args / picker module missing
    //   -2 → this app already has a dialog open
    mem fn npk_pick(mode: i32, start_ptr: i32, start_len: i32, suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32;

    // npk_pick_result(path_ptr, path_len) -> 0 / -1
    // Report the picked path back to whoever opened this dialog. An empty
    // path means the user cancelled.
    //
    // Authorisation is structural: the caller's window id must be one the
    // kernel itself registered as a picker in `npk_pick`. An ordinary app
    // calling this finds no session and gets -1, so it cannot forge a
    // "the user chose this file" claim for another app.
    mem fn npk_pick_result(path_ptr: i32, path_len: i32) -> i32;

    // npk_window_set_close_guard(on) -> 0 / -1
    // Ask to be consulted before this window closes. A guarded window gets
    // `Event::CloseRequest` on Mod+Q / the title-bar X instead of vanishing,
    // so an app with unsaved work can prompt.
    //
    // Not a veto: asking again, or staying silent for a few seconds, closes
    // it anyway. Apps that don't opt in are unaffected.
    ctx fn npk_window_set_close_guard(on: i32) -> i32;

    // npk_pick_mkdir(path_ptr, path_len) -> 0 / -1
    // Create a directory on behalf of an open file dialog.
    //
    // This exists so the picker can offer "New folder" without holding
    // WRITE. Giving it WRITE would hand the module that browses every
    // file the right to overwrite them too — the one thing the portal is
    // built to avoid. So the capability is this single verb instead:
    // create a directory, nothing else. No writing files, no deleting,
    // no renaming.
    //
    // Authorised exactly like `npk_pick_result` — the caller's window
    // must be one the kernel itself registered as a picker. `sys/` stays
    // off limits regardless.
    mem fn npk_pick_mkdir(path_ptr: i32, path_len: i32) -> i32;

    // npk_scene_commit(ptr, len) -> i32
    // Widget pipeline: the WASM app hands the kernel a version-
    // prefixed postcard-serialized Widget tree. Compositor does the
    // rest (version check, deserialize, layout, raster, per-window
    // scene store, shade render). Requires RENDER right.
    //
    // Return protocol mirrors shade::widgets::scene_commit:
    //   >0 → new widget window created, id returned (caller should
    //        treat return value as opaque)
    //   0  → reused existing widget window
    //   -1 → version mismatch / cap denied / bad payload
    //   -2 → postcard decode failure
    //   -3 → shade couldn't allocate a window
    mem fn npk_scene_commit(ptr: i32, len: i32) -> i32;

    // npk_canvas_commit(canvas_id, ptr, len, width, height) -> 0 / -1
    // Escape hatch: upload a raw BGRA32 bitmap into the app's
    // `Widget::Canvas` with the matching id. CANVAS-gated. The app must
    // already own a widget window (commit a scene first) — the bitmap is
    // keyed by (window_id, canvas_id); the render walker blits it
    // contain-fit into the canvas rect on the next rasterise.
    mem fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, width: i32, height: i32) -> i32;

    // npk_canvas_commit_yuv(canvas_id, y, u, v, ys, cs, w, h, flags) -> 0 / -1
    // The same escape hatch for a planar 4:2:0 frame — a video decoder
    // hands over its own planes and the blit converts, at destination
    // size and natively. CANVAS-gated exactly like the BGRA form.
    mem fn npk_canvas_commit_yuv(canvas_id: i32, y_ptr: i32, u_ptr: i32, v_ptr: i32, ys: i32, cs: i32, width: i32, height: i32, flags: i32) -> i32;

    // npk_screen_size() -> (width << 16) | height, or 0 on error.
    // Allowed for RENDER (overlay sizing) OR CAPTURE (screenshot tool
    // sizing its capture buffer — it has no RENDER in full-screen mode).
    // (Screens are well under 65535 px/side.)
    ctx fn npk_screen_size() -> i32;

    // npk_ticks() -> milliseconds since boot (monotonic), or -1.
    //
    // A clock, not a calendar: no wall time, no timezone, nothing that
    // identifies the machine — so it needs no capability, like the theme
    // query. Resolution is the 100 Hz timer, i.e. 10 ms steps; enough to
    // attribute phases of a page load, not enough to time a single glyph.
    // Backs `setTimeout`/`requestAnimationFrame` (docs/spec/BROWSER.md §10).
    ctx fn npk_ticks() -> i64;

    // npk_now_us() -> microseconds since boot, from the TSC, or 0.
    //
    // Same "clock, not calendar" argument as npk_ticks, so equally ungated, but
    // fine enough to time one pass of a driver loop, which 10 ms steps cannot.
    ctx fn npk_now_us() -> i64;

    // npk_unix_time() -> seconds since the epoch, UTC, or 0 if the clock is
    // not readable. Ungated, like npk_ticks: the wall clock is not a secret —
    // it is on the bar and stamped into every npkFS entry. `npk_ticks` cannot
    // stand in for it, because it restarts at every boot and a cookie's
    // `Expires` is an absolute date.
    ctx fn npk_unix_time() -> i64;

    // npk_theme_token(token_id) -> RGBA u32 (0xAARRGGBB) for the active theme
    // (light/dark aware), or 0 for an unknown token. RENDER-gated. Lets an app
    // that paints its own surface (e.g. the browser's Canvas) match the theme's
    // colours instead of hardcoding them.
    ctx fn npk_theme_token(token_id: i32) -> i32;

    // npk_canvas_rect(canvas_id, out_ptr) -> 0 / -1
    // Writes the canvas widget's actual laid-out rect as 4 little-endian i32
    // [x, y, w, h] into out_ptr (16 bytes), so an app can paint its canvas
    // 1:1 (no contain-fit scaling) and map click coordinates into content
    // space. RENDER-gated. Returns -1 until the canvas has been laid out once
    // (commit a scene with the Canvas first, then query on the next frame).
    mem fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;

    // npk_cursor_pos() -> (x << 16) | y, or -1
    //
    // Screen coordinates, the same space `Event::MouseMove` and
    // `Event::MouseButton` report. RENDER-gated and focus-gated: an app
    // may learn where the pointer is only while it holds focus, so a
    // background module cannot watch the mouse.
    //
    // Exists because `Event::Wheel` carries no position — an app that
    // wants to zoom towards the pointer has to ask for it.
    ctx fn npk_cursor_pos() -> i32;

    // npk_screen_flash() -> 0 or -1
    // CAPTURE-gated: same right as reading the screen, because this is
    // the acknowledgement for exactly that act. Paints a white wash over
    // the finished frame for ~150 ms. The caller is expected to capture
    // first and flash after, so the wash can never be in the shot; the
    // compositor also draws it last, after every window.
    ctx fn npk_screen_flash() -> i32;

    // npk_capture_screen(buf_ptr, buf_max) -> bytes_written or -1
    // CAPTURE-gated (screen-scrape — only the screenshot tool holds it).
    // Copies the composited front framebuffer as tightly-packed BGRA32
    // (width*height*4) into the app buffer. The app then PNG-encodes /
    // crops it itself; the kernel only hands over the raw pixels.
    mem fn npk_capture_screen(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_event_poll(buf_ptr, buf_max) -> i32
    // Non-blocking: pop one event from this app's widget-window
    // queue, postcard-encode it into the supplied WASM buffer.
    //   >0 → encoded byte count
    //   0  → queue empty (app should sleep / yield)
    //   -1 → no widget window, cap denied, or buffer too small
    mem fn npk_event_poll(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_list_modules(buf_ptr, buf_max) -> i32
    // Writes a NUL-separated list of module names from `sys/wasm/*` into
    // the caller's buffer. Returns bytes written, or -1 on cap denied /
    // buffer too small. The trailing entry is not terminated — caller
    // splits on 0x00.
    //
    // RENDER-gated because only GUI apps (drun) need it.
    mem fn npk_list_modules(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_app_meta(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
    // Returns only the `.npk.app_meta` custom-section payload of the module
    // `sys/wasm/<name>`, extracted kernel-side. Launchers (drun/dock) read an
    // app's icon/name/description with this without fetching the whole
    // module, which can be several MB (the section sits at the end).
    // `name` is confined to a bare child of `sys/wasm/` (no path traversal).
    // RENDER-gated like npk_list_modules.
    mem fn npk_app_meta(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_spawn_module(name_ptr, name_len) -> i32
    // Launch `sys/wasm/<name>` in a fresh terminal window and focus it.
    //
    // Modelled on `Mod+Enter` + `run <name>` — the user-expected flow
    // when drun picks a module. Terminal-kind apps (top, debug) print
    // into the new loop's terminal; widget-kind apps can convert their
    // window via `npk_window_set_overlay` from `_start`.
    //
    //   0  → spawn accepted
    //   -1 → cap denied / bad args / module not found / compositor
    //        unavailable (no free terminal slot)
    mem fn npk_spawn_module(name_ptr: i32, name_len: i32) -> i32;

    // npk_run_intent(verb_ptr, verb_len) -> i32
    // Trigger a built-in system intent that isn't a WASM module — the
    // launcher path for microvm-backed apps. Currently `browser` is
    // the only verb. Returns 0 on accepted, -1 on cap denied / unknown
    // verb / unsupported on the cooperative path.
    //
    // Safe from a worker core: vm_open with a dedicated VM core
    // is pure atomic + mutex (stash PENDING_VM → return; the
    // dedicated core picks it up via vm_core_serve and runs the
    // entire VM lifecycle on itself). Cooperative path (≤2 cores)
    // needs Core-0 BSP state for VMXON, so this rejects there.
    mem fn npk_run_intent(verb_ptr: i32, verb_len: i32) -> i32;

    // npk_window_set_overlay(w, h) -> i32
    // Mark the calling app's widget window as a centred overlay of the
    // requested size. Removes the window from the tiling grid (if it
    // was part of it), re-centres it, and requests re-render.
    //
    // If the app hasn't created its widget window yet (widget_window_id
    // == 0), this call also creates the window — title is the module
    // name recorded at spawn time. First caller "wins" the window;
    // subsequent calls just reconfigure.
    //
    // Returns 0 on success, -1 on cap denied / compositor unavailable.
    ctx fn npk_window_set_overlay(w: i32, h: i32) -> i32;

    // npk_window_set_modal(modal: i32) -> i32
    // Toggle the modal flag on the calling app's widget window. While
    // any window is modal, shade-action dispatch suppresses focus-shift
    // / tiling shortcuts (see handle_action in shade/mod.rs).
    //
    // Returns 0 on success, -1 if the app has no widget window yet /
    // cap denied.
    ctx fn npk_window_set_modal(modal: i32) -> i32;

    // npk_window_set_overlay_at(x, y, w, h) -> i32
    // Like npk_window_set_overlay but positions the overlay's top-left at
    // (x, y) instead of centring it — for corner-anchored dropdowns (e.g.
    // the volume slider under the bar). Creates/promotes + focuses the
    // caller's widget window, same as the centred overlay path.
    //
    // Returns 0 on success, -1 on cap denied / bad args / no compositor.
    ctx fn npk_window_set_overlay_at(x: i32, y: i32, w: i32, h: i32) -> i32;

    // npk_window_set_light_dismiss(on: i32) -> i32
    // Opt the caller's widget window into light-dismiss: the compositor
    // closes it when a click lands outside it (transient overlays like the
    // volume slider). Off by default, so other overlays (loft, drun) are
    // unaffected. Returns 0 on success, -1 if no widget window / cap denied.
    ctx fn npk_window_set_light_dismiss(on: i32) -> i32;

    // npk_window_set_clipboard_sink() -> i32
    // Opt the caller's widget window into Ctrl+C/X/V delivery as
    // Event::Clipboard when a focused text widget can't act on the chord
    // (copy/cut with no selection, paste into an empty single-line Input).
    // Used by file managers so the shortcuts drive file operations without
    // stealing text copy/paste from other apps. Returns 0 / -1.
    ctx fn npk_window_set_clipboard_sink() -> i32;

    // npk_window_set_dock(w, h) -> i32
    // Turn the calling app's widget window into a bottom auto-hide dock:
    // overlay (no tiling strut), never modal, never focused on reveal,
    // global across workspaces. Starts hidden; the compositor slides it
    // in when the cursor holds the bottom edge. Like set_overlay but
    // bottom-anchored instead of centred, and it does not grab focus.
    //
    // Returns 0 on success, -1 on cap denied / bad args / no compositor.
    ctx fn npk_window_set_dock(w: i32, h: i32) -> i32;

    // npk_window_set_panel(edge, behavior, w, h) -> i32
    // Generalised edge panel (see docs/spec/PANEL.md): edge 0=Bottom 1=Top,
    // behavior 0=AutoHide overlay (dock) 1=Strut (bar). Creates/promotes
    // the caller's widget window without grabbing focus (like the dock),
    // then hands it to the compositor's panel config. `set_dock` above is
    // the (Bottom, AutoHide) wrapper of this.
    //
    // Returns 0 on success, -1 on cap denied / bad args / no compositor.
    ctx fn npk_window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) -> i32;

    // npk_bar_state(buf, max) -> i32
    // Live state for the bar app: "HH:MM\n<ws_count>\n<ws_active>\n<title>"
    // (clock already timezone-adjusted). Returns bytes written, -1 on
    // cap / args / buffer too small.
    mem fn npk_bar_state(buf_ptr: i32, max: i32) -> i32;

    // npk_window_titles(buf, max) -> i32
    // One line per open app window: "<flags>\t<workspace>\t<title>", flags
    // being a decimal bitmask (1 = focused, 2 = on the active workspace).
    // Panels and overlays are excluded. The dock derives its running/active
    // indicators from this, the bar its occupied-workspace hints; the
    // kernel stays free of app names.
    mem fn npk_window_titles(buf_ptr: i32, max: i32) -> i32;

    // npk_battery() -> i32 — battery state for the bar plugin. Returns -1
    // when no battery is known (desktops/QEMU → segment stays empty), else
    // (status << 8) | percent, with status 0=discharging 1=charging 2=full
    // 3=plugged-idle and percent in 0..=100. Prefers the AML driver's report
    // (aml.wasm, vendor-independent via _BST/_BIF); falls back to the
    // standardised SBS-over-SMBus path for SBS laptops.
    ctx fn npk_battery() -> i32;

    // ── AML battery driver (aml.wasm) host functions, HARDWARE-gated ─────
    // npk_acpi_dsdt(buf_ptr, buf_max) -> i32: copy the DSDT (firmware AML)
    // into the caller's buffer; returns the DSDT length. If it exceeds
    // buf_max nothing is copied (caller sizes its buffer up). -1 on error.
    mem fn npk_acpi_dsdt(buf_ptr: i32, buf_max: i32) -> i32;

    // npk_acpi_mem_read(hi, lo) -> byte, or -1 (RAM / no right / unmapped)
    ctx fn npk_acpi_mem_read(hi: i32, lo: i32) -> i32;

    // npk_acpi_table(sig, index, buf_ptr, buf_max) -> len. The n-th table
    // with this signature; `sig` is the four characters, little-endian.
    mem fn npk_acpi_table(sig: i32, index: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_mmio_map_phys(hi, lo, pages) -> handle, or -1. For hardware not on
    // PCI (e.g. the FCH I2C controller). Rights and the RAM/APIC exclusion
    // are enforced in host_core.
    ctx fn npk_mmio_map_phys(hi: i32, lo: i32, pages: i32) -> i32;

    // npk_pointer_inject(dx, dy, buttons, scroll, hscroll) -> 0, or -1 without the right.
    ctx fn npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32) -> i32;

    // npk_ec_query() -> query number, or -1 when nothing is pending
    ctx fn npk_ec_query() -> i32;

    // npk_ec_read(addr) -> i32: read one EC-RAM byte (0..255) or -1.
    ctx fn npk_ec_read(addr: i32) -> i32;

    // npk_ec_write(addr, val) -> i32: firmware-directed EC write (BSEL etc.).
    // 0 on success, -1 on error.
    ctx fn npk_ec_write(addr: i32, val: i32) -> i32;

    // npk_battery_report(packed): the AML driver pushes the decoded battery
    // state ((status<<8)|percent, or -1 for absent) into the kernel cache
    // that npk_battery() returns.
    ctx fn npk_battery_report(packed: i32);

    // npk_battery_detail(rate, remaining, full, voltage_mv, unit): the raw
    // _BST/_BIF figures behind the percentage, for `battery`.
    ctx fn npk_battery_detail(rate: i32, remaining: i32, full: i32, voltage_mv: i32, unit: i32);

    // ── Audio mailbox + mixer ────────────────────────────────────────────
    // Apps push PCM (S16LE / 48 kHz / stereo) into per-slot rings; the HDA
    // driver pulls a mixed stream via npk_audio_poll_mix. Ungated: audio
    // playback is not a security boundary, and the kernel holds no HDA
    // knowledge — it just shuttles + sum-mixes bytes.

    // npk_audio_open() -> slot index, or -1 if no slot free.
    ctx fn npk_audio_open() -> i32;

    // npk_audio_close(slot) -> 0.
    ctx fn npk_audio_close(slot: i32) -> i32;

    // npk_audio_submit(slot, ptr, len) -> bytes accepted, or -1 on bad args.
    mem fn npk_audio_submit(slot: i32, ptr: i32, len: i32) -> i32;

    // npk_audio_buffered(slot) -> bytes still in the ring, -1 if closed.
    // The honest play clock; see host_core for why the wall clock is not one.
    ctx fn npk_audio_buffered(slot: i32) -> i32;

    // npk_audio_poll_mix(ptr, max) -> bytes written (driver side).
    mem fn npk_audio_poll_mix(ptr: i32, max: i32) -> i32;

    // npk_audio_set_volume(pct) -> 0; npk_audio_get_volume() -> 0..=100.
    ctx fn npk_audio_set_volume(pct: i32) -> i32;
    ctx fn npk_audio_get_volume() -> i32;

    // npk_workspace_switch(n) -> i32 — switch to workspace n (bar clicks).
    ctx fn npk_workspace_switch(n: i32) -> i32;

    // npk_power() -> i32 — ACPI S5 power-off (bar power button). Does not
    // return on success.
    ctx fn npk_power() -> i32;

    // npk_fs_list(prefix_ptr, prefix_len, out_ptr, out_cap, recursive) -> i32
    // Enumerate npkFS keys under `prefix`. If recursive=0, only direct
    // children are returned (keys that contain no '/' after the prefix,
    // plus the unique directory bucket names that do). If recursive=1,
    // every key under the prefix is emitted verbatim.
    //
    // Wire format of the output buffer — one entry per line, separated
    // by '\n' (no trailing newline after the last):
    //   <name>\0<size_le_u64:8>\0<is_dir_u8>\0<mtime_le_u64:8>
    // - <name> is relative to `prefix` (prefix itself + trailing slash
    //   stripped). For a synthetic directory entry (first path component
    //   encountered in recursive scan), size=0 and is_dir=1.
    // - Size is little-endian 8 bytes. is_dir is 0 or 1.
    // - mtime is UTC seconds since the Unix epoch (LE u64). Zero means
    //   "unknown" (RTC was unreadable when this entry was created).
    //   Synthetic directory entries from recursive descent inherit
    //   mtime=0; only stored TreeEntry instances carry real values.
    //
    // Returns bytes written, 0 if prefix is empty, -1 on cap / args /
    // truncation (buffer too small to fit the full listing).
    mem fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;

    // npk_fs_stat(name_ptr, name_len, out_ptr) -> i32
    // Write 17 bytes into out_ptr:
    //   size_le_u64 (8) + is_dir_u8 (1) + mtime_le_u64 (8).
    // Returns 17 on success, 0 if no entry, -1 on cap / args.
    // mtime is UTC seconds since the Unix epoch — zero means unknown.
    mem fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;

    // npk_fs_delete(name_ptr, name_len) -> i32
    // Delete a single npkFS key. WRITE-gated. Returns 0 on success,
    // -1 on cap / not found / fs error.
    mem fn npk_fs_delete(name_ptr: i32, name_len: i32) -> i32;

    // npk_fs_rename(old_ptr, old_len, new_ptr, new_len) -> i32
    // Move/rename a single npkFS key (files and whole directories).
    // Content-addressed, so even a directory move is O(1). WRITE-gated;
    // neither path may touch the module store. Returns 0 / -1.
    mem fn npk_fs_rename(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;

    // npk_fs_copy(old_ptr, old_len, new_ptr, new_len) -> i32
    // Copy a single npkFS key (files and whole directories). Shares the
    // source's content hash — no data duplication. WRITE-gated; neither
    // path may touch the module store. Returns 0 / -1.
    mem fn npk_fs_copy(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;

    // npk_close_widget() -> i32
    // Close the calling app's own widget window. The worker then falls
    // out of its `_start` loop by its own logic; this host fn only tears
    // down the window + scene + event queue. Returns 0 on success,
    // -1 if the app doesn't own a widget window.
    ctx fn npk_close_widget() -> i32;

    // npk_get_fb_size() -> (width << 16) | height
    ctx fn npk_get_fb_size() -> i64;

    // npk_set_wallpaper(ptr, len, width, height) -> 0 or -1
    // Receives raw BGRA pixel data, sets it as the compositor wallpaper.
    mem fn npk_set_wallpaper(ptr: i32, len: i32, width: i32, height: i32) -> i32;

    // npk_set_theme(ptr) -> 0 or -1
    // Receives 16 u32 colors (64 bytes), sets as theme palette.
    mem fn npk_set_theme(ptr: i32) -> i32;

    // npk_sys_info(key) -> i64 — system information for apps (e.g. top)
    // Keys: 0=cores, 1=uptime_secs, 2=free_mb, 3=heap_used, 4=heap_total,
    //        5=tasks_spawned, 6=tasks_completed, 7=steals, 8=workers,
    //        9=has_mwait, 10=tsc_mhz, 11=queue_len(core N, pass core in high bits)
    ctx fn npk_sys_info(key: i32) -> i64;

    // npk_sleep(ms) -> 0 — sleep for N milliseconds.
    // Parks this app's fiber and yields the worker core to the per-core
    // scheduler, which runs the core's other ready fibers meanwhile, so many
    // apps multiplex over a few workers instead of each pinning a core
    // (docs/plan/SCHEDULER_FIBERS.md). The fiber resumes once the deadline
    // passes.
    ctx fn npk_sleep(ms: i32) -> i32;

    // npk_input_poll() -> key or -1 — non-blocking read from per-app buffer
    ctx fn npk_input_poll() -> i32;

    // npk_input_wait(timeout_ms) -> key or -1 — blocking wait with timeout
    // Spins on worker core checking per-app key buffer + TSC deadline.
    // Flushes busy-TSC and marks core idle during wait for accurate CPU usage.
    ctx fn npk_input_wait(timeout_ms: i32) -> i32;

    // npk_sci_arm(gpe) -> vector | -1 and npk_sci_service() -> mask: the
    // ACPI SCI for the AML driver's EC (drivers/sci.rs).
    ctx fn npk_sci_arm(gpe: i32) -> i32;
    ctx fn npk_sci_service() -> i32;

    // npk_irq_register_gsi(gsi, flags) -> vector | -1 — a non-PCI device's
    // I/O APIC line (flags: 1 level, 2 active-low). HARDWARE-gated.
    ctx fn npk_irq_register_gsi(gsi: i32, flags: i32) -> i32;

    // npk_wait(mask, timeout_ms) -> fired bits (0 = timeout). Parks the
    // app's fiber until an input event arrives (mask bit 1) or the timeout
    // passes; timeout < 0 waits without one. See host_core::npk_wait.
    ctx fn npk_wait(mask: i32, timeout_ms: i32) -> i32;

    // npk_clear() — clear the app's terminal
    ctx fn npk_clear();

    // ── Terminal Stream Sink (for remote debug mirroring) ─────────

    // npk_self_terminal() -> terminal_idx of this WASM task
    ctx fn npk_self_terminal() -> i32;

    // npk_stream_open(idx) -> 0 ok, -1 error
    ctx fn npk_stream_open(idx: i32) -> i32;

    // npk_stream_read(idx, buf_ptr, buf_len) -> bytes read (>=0) or -1 on error
    mem fn npk_stream_read(idx: i32, buf_ptr: i32, buf_len: i32) -> i32;

    // npk_stream_close(idx) -> 0
    ctx fn npk_stream_close(idx: i32) -> i32;

    // npk_key_inject(byte) -> 0
    // Injects a raw byte into the global keyboard buffer. Routes to the
    // currently-focused window's intent session. Used by debug.wasm.
    ctx fn npk_key_inject(byte: i32) -> i32;

    // ── TCP/TLS socket host functions ────────────────────────────

    // ── npk_tls_* ────────────────────────────────────────────────────────
    //
    // Must be registered in both ABI paths (here and forge_glue), otherwise a
    // module works under one engine and fails under the other.
    mem fn npk_tls_connect(host_ptr: i32, host_len: i32, port: i32) -> i32;
    mem fn npk_tls_send(handle: i32, buf_ptr: i32, buf_len: i32) -> i32;
    mem fn npk_tls_recv(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;
    ctx fn npk_tls_close(handle: i32) -> i32;

    // npk_tcp_connect(ip_packed, port) -> handle (>=0) or -1 on error.
    // ip_packed = (a << 24) | (b << 16) | (c << 8) | d.
    //
    // Non-blocking: returns as soon as the handshake is started. Ask
    // `npk_tcp_status` until it answers; `npk_tcp_send` refuses until then.
    // A module is a fiber, so blocking here would stall every other fiber
    // on its worker core, drivers included.
    ctx fn npk_tcp_connect(ip_packed: i32, port: i32) -> i32;

    // npk_tcp_status(handle) -> 1 established, 0 still handshaking, -1 failed.
    // The polling half of the non-blocking connect above. The module sleeps
    // between calls; Core 0's run loop drives the stack meanwhile.
    ctx fn npk_tcp_status(handle: i32) -> i32;

    // npk_tcp_send(handle, buf_ptr, buf_len) -> 0 ok, -2 retry later, -1 error
    mem fn npk_tcp_send(handle: i32, buf_ptr: i32, buf_len: i32) -> i32;

    // npk_tcp_recv(handle, buf_ptr, buf_max) -> bytes read (0 = none available), -1 on error
    mem fn npk_tcp_recv(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;

    // npk_tcp_close(handle) -> 0. Sends the FIN and returns; the graceful
    // wait is the kernel's job, not a module's, since spinning here would
    // freeze the worker core.
    ctx fn npk_tcp_close(handle: i32) -> i32;

    // npk_debug_target_ip() -> packed IP (0 if unset)
    ctx fn npk_debug_target_ip() -> i32;

    // npk_debug_target_port() -> port (0 if unset)
    ctx fn npk_debug_target_port() -> i32;

    // ── Hardware Driver Host Functions ────────────────────────────

    // npk_pci_bind(vendor_id, device_id) -> 0=ok, -1=not found, -2=denied
    ctx fn npk_pci_bind(vendor: i32, device: i32) -> i32;

    // npk_pci_bind_class(class, subclass) -> 0=ok, -1=not found, -2=denied
    ctx fn npk_pci_bind_class(class: i32, subclass: i32) -> i32;

    // npk_pci_bind_class_n(class, subclass, index) -> 0=ok, -1=not found, -2=denied
    ctx fn npk_pci_bind_class_n(class: i32, subclass: i32, index: i32) -> i32;

    // npk_pci_read_config(offset) -> u32 value or -1
    ctx fn npk_pci_read_config(offset: i32) -> i32;

    // npk_pci_write_config(offset, value) -> 0 or -1
    ctx fn npk_pci_write_config(offset: i32, value: i32) -> i32;

    // npk_pci_enable_bus_master() -> 0 or -1
    ctx fn npk_pci_enable_bus_master() -> i32;

    // ── Device-interrupt ABI (MSI-X → LAPIC → fiber wake) ───────────────
    //
    // Lets a WASM driver go IRQ-driven instead of `npk_sleep`-polling: bind a
    // device, `npk_irq_register` its MSI-X entry once, then loop
    //   since = npk_irq_arm(vec); <enable/submit device work>;
    //   npk_irq_wait(vec, since, timeout); <service>
    // The driver's fiber parks until the device fires; the IRQ wakes its core.
    // The driver still enables the device's own interrupt source via its MMIO
    // (e.g. a queue's IRQ-enable) using the existing npk_mmio_* fns.

    // npk_irq_register(entry) -> LAPIC vector (>=0), or -1. Programs the bound
    // device's MSI-X table `entry` to deliver to this driver's core.
    ctx fn npk_irq_register(entry: i32) -> i32;

    // npk_irq_arm(vector) -> fired-count snapshot, or -1 on a bad vector. Call
    // before submitting/enabling the device work that triggers the IRQ; pass
    // the result to npk_irq_wait. Also routes the IRQ to the calling core.
    ctx fn npk_irq_arm(vector: i32) -> i64;

    // npk_irq_wait(vector, since, timeout_ms) -> 1 fired, 0 timeout, -1 bad arg.
    // Parks the driver's fiber until the device IRQ advances the fired-count
    // past `since`, or `timeout_ms` elapses (defaults to 1000 if <= 0).
    ctx fn npk_irq_wait(vector: i32, since: i64, timeout_ms: i32) -> i32;

    // npk_mmio_map_bar(bar_index, page_count) -> handle or -1
    //
    // Sizes the BAR first and clamps `pages` to the actual BAR size. This
    // prevents drivers from mapping past the end of a BAR into whatever PCI
    // address space follows (usually another device's BAR), which would
    // silently corrupt that device or generate UR responses.
    ctx fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;

    // npk_mmio_read32(handle, offset) -> u32
    ctx fn npk_mmio_read32(handle: i32, offset: i32) -> i32;

    // npk_mmio_write32(handle, offset, value) -> 0 or -1
    ctx fn npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;

    // npk_mmio_read16(handle, offset) -> u16 as i32
    ctx fn npk_mmio_read16(handle: i32, offset: i32) -> i32;

    // npk_mmio_write16(handle, offset, value) -> 0 or -1
    // True 16-bit MMIO write — required for split registers like RX/TX BD IDX
    // (HOST_IDX[15:0] + HW_IDX[31:16]). A 32-bit RMW would clobber HW_IDX.
    ctx fn npk_mmio_write16(handle: i32, offset: i32, value: i32) -> i32;

    // npk_mmio_read8(handle, offset) -> u8 as i32
    // npk_mmio_write8(handle, offset, value) -> 0 or -1
    // True 8-bit MMIO. rtw88 (RTL8822CE) runs its power sequence as a
    // read8/write8 interpreter and addresses registers like REG_SYS_FUNC_EN+1
    // as a single byte. A 32-bit RMW touches three neighbouring bytes and is
    // a different operation; Linux picks the width on purpose.
    ctx fn npk_mmio_read8(handle: i32, offset: i32) -> i32;
    ctx fn npk_mmio_write8(handle: i32, offset: i32, value: i32) -> i32;

    // npk_mmio_read64(handle, offset) -> i64
    ctx fn npk_mmio_read64(handle: i32, offset: i32) -> i64;

    // npk_mmio_write64(handle, offset, value) -> 0 or -1
    ctx fn npk_mmio_write64(handle: i32, offset: i32, value: i64) -> i32;

    // npk_dma_alloc_below(page_count, limit_mb) -> handle or -1
    // The driver names the upper limit itself. `allocate_contiguous_below`
    // searches from the top, so a 4 GB limit always lands right below the
    // PCI MMIO hole — on AMD platforms exactly where TSEG/DPR rejects device
    // DMA while the CPU can still read and write it.
    ctx fn npk_dma_alloc_below(pages: i32, limit_mb: i32) -> i32;

    // npk_dma_alloc(page_count) -> handle or -1
    ctx fn npk_dma_alloc(pages: i32) -> i32;

    // npk_dma_phys_addr(handle) -> physical address as i64
    ctx fn npk_dma_phys_addr(handle: i32) -> i64;

    // npk_dma_read(handle, dma_offset, wasm_ptr, len) -> 0 or -1
    mem fn npk_dma_read(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;

    // npk_dma_write(handle, dma_offset, wasm_ptr, len) -> 0 or -1
    mem fn npk_dma_write(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;

    // npk_dma_read32(handle, offset) -> u32
    ctx fn npk_dma_read32(handle: i32, offset: i32) -> i32;

    // npk_dma_write32(handle, offset, value) -> 0 or -1
    ctx fn npk_dma_write32(handle: i32, offset: i32, value: i32) -> i32;

    // npk_memory_fence() -> 0
    ctx fn npk_memory_fence() -> i32;

    // npk_netdev_register(mac_ptr) -> 0 or -1
    mem fn npk_netdev_register(mac_ptr: i32) -> i32;

    // ── WiFi-class control channel (docs/spec/WIFI_CLASS_ABI.md) ───────────────────
    // A kernel-mediated mailbox pair routing opaque control messages between
    // the vendor driver (wifi_*.wasm) and the supplicant (wifid.wasm). The
    // kernel carries bytes only — no WPA / vendor knowledge. Manager side is
    // NETCTL-gated (only wifid, which declares it in .npk.caps); driver side
    // is gated by being a bound driver (hw state present), like the other
    // device host-fns.

    // npk_wifi_send_cmd(buf_ptr, len) -> 0 / -1 — manager enqueues a command.
    mem fn npk_wifi_send_cmd(buf_ptr: i32, len: i32) -> i32;

    // npk_wifi_poll_event(buf_ptr, max) -> len / -1 — manager dequeues an event.
    mem fn npk_wifi_poll_event(buf_ptr: i32, max: i32) -> i32;

    // npk_wifi_poll_cmd(buf_ptr, max) -> len / -1 — driver dequeues a command.
    mem fn npk_wifi_poll_cmd(buf_ptr: i32, max: i32) -> i32;

    // npk_wifi_send_event(buf_ptr, len) -> 0 / -1 — driver enqueues an event.
    mem fn npk_wifi_send_event(buf_ptr: i32, len: i32) -> i32;

    // npk_driver_report(buf_ptr, len) -> 0 / -1 — a bound driver publishes a
    // short plain-text status snapshot, read back with the `wlan` intent. The
    // kernel stores the bytes and a timestamp and never parses them: what is
    // worth reporting is device knowledge, and that stays in the driver.
    mem fn npk_driver_report(buf_ptr: i32, len: i32) -> i32;

    // ── WiFi/NIC data path (driver ↔ kernel IP stack via the netdev mailbox) ─

    // npk_netdev_submit_rx(buf_ptr, len) -> 0 / -1 — driver hands a received
    // Ethernet frame to the kernel network stack.
    mem fn npk_netdev_submit_rx(buf_ptr: i32, len: i32) -> i32;

    // npk_netdev_rx_deliver(buf_ptr, len) -> 0 / -1 — driver delivers a received
    // frame straight into the IP stack from its own fiber (the NAPI topology:
    // drain → stack in one context, no relay-ring + Core-0 hop). Falls back to
    // the ring internally if Core 0 holds the drain guard. Preferred over
    // npk_netdev_submit_rx for the hot path.
    mem fn npk_netdev_rx_deliver(buf_ptr: i32, len: i32) -> i32;

    // npk_netdev_poll_tx(buf_ptr, max) -> len / -1 — driver fetches the next
    // frame the kernel wants transmitted (-1 when none / buffer too small).
    mem fn npk_netdev_poll_tx(buf_ptr: i32, max: i32) -> i32;

    // npk_netdev_set_link(up) -> 0 — the single-flag form. Kept for drivers
    // that know only "usable / not usable"; it reports `up` as carrier with no
    // dormant phase.
    ctx fn npk_netdev_set_link(up: i32) -> i32;

    // npk_netdev_set_link_state(carrier, dormant) -> 0 — the RFC 2863 pair
    // Linux keeps (`rfc2863_policy`): `carrier` = the association exists,
    // `dormant` = it exists but is not usable yet (WPA not done). operstate
    // is UP only when carrier && !dormant, so an authorization phase does
    // not look like a lost link.
    ctx fn npk_netdev_set_link_state(carrier: i32, dormant: i32) -> i32;
}
