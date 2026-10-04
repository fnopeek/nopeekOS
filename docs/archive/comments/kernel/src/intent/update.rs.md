# `kernel/src/intent/update.rs` @ 5e0102684

## L1-4 · `use crate::kprintln;`

```
//! OTA update intent.
//!
//! Downloads kernel from GitHub, verifies ECDSA P-384 signature,
//! writes to ESP FAT32 partition.
```

## L12-20 · `const MAX_KERNEL_SIZE: usize = 64 * 1024 * 1024;`

```
/// Hard ceiling on a kernel image we are willing to buffer. NOT the download
/// bound — that comes from the signed manifest (see below), so this only has
/// to be "implausible", not "current size plus guesswork".
///
/// It used to be 4 MiB and used directly as the download cap. When the kernel
/// crossed 4 MiB the fetch was silently truncated there, and OTA failed with a
/// confusing `Size mismatch` — the updater on the device could no longer
/// install any kernel, including the one that fixes this. Deriving the bound
/// from the manifest means the cap can never again drift away from reality.
```

## L24-28 · `const MAX_ASSET_SIZE: usize = 512 * 1024 * 1024;`

```
/// 512 MB ceiling for OTA assets. The userspace bundle with Mesa/Wayland
/// runs ~270 MB; raw-githubusercontent caps at ~50–100 MB per file, so
/// anything above ~32 MB ships via GitHub Releases (asset manifest carries
/// an explicit `url=` line for those; redirect-following lives in
/// `https_get`).
```

## L32-34 · `struct AssetSpec {`

```
/// Mapping from asset-manifest section header to (remote filename,
/// npkFS path). Keep in sync with `build.sh` ASSET_MANIFEST writer
/// and `kernel/src/install_data/assets/mod.rs` BUNDLED entries.
```

## L44-46 · `AssetSpec { section: "font:LICENSE-Inter",   remote_filename: "LICENSE-Inter.txt",        npkfs_path: "sys/fonts/LICENSE`

```
// Both faces are SIL OFL 1.1: the licence must accompany every copy,
// so an OTA-updated system pulls it alongside the font rather than
// only fresh installs getting it from the bundled assets.
```

## L52-63 · `AssetSpec { section: "microvm:userspace",   remote_filename: "microvm-userspace.cpio.gz", npkfs_path: "sys/microvm/users`

```
// Optional userspace bundle — Alpine minirootfs + busybox + (future)
// Wayland/Mesa/LibreWolf. Built by `microvm-userspace/build.sh`.
// Distinct from `microvm:initramfs` (which is just our PID-1, always
// present). If a release ships this asset, OTA pulls it; if not, the
// entry is absent in the asset manifest and we keep whatever's
// already installed (or nothing).
//
// Small bundles (<~30 MB) live in `release/assets/` on the `main`
// branch and ship via raw.githubusercontent.com. Larger bundles
// (Mesa+Wayland is ~270 MB) live on GitHub Releases — the asset
// manifest carries a `url=` override per entry and `https_get`
// follows the 302 redirect chain to objects.githubusercontent.com.
```

## L65-68 · `AssetSpec { section: "microvm:userspace-sqfs", remote_filename: "microvm-userspace.sqfs",  npkfs_path: "sys/microvm/user`

```
// Squashfs form of the userspace bundle — read-only, mounted by
// PID-1 from /dev/vdb (slot-5 virtio-blk) instead of unpacked into
// a tmpfs initramfs. The RAM-efficient daily-driver path; supersedes
// the cpio entry above once it's the only shipped form.
```

## L70-77 · `AssetSpec { section: "python:stdlib",       remote_filename: "python313.zip",             npkfs_path: "sys/python/lib/py`

```
// CPython's standard library, as one zip. Stored uncompressed on
// purpose: this interpreter has no zlib, so a deflated zip raises
// ZipImportError at the first import. Costs ~5 MB over the wire and
// saves decompressing on every import — a fair trade on a machine
// where the interpreter is already the slow part.
//
// Not bundled into the installer: like microvm:userspace, Python is
// something you fetch, not something every USB stick carries.
```

## L85-87 · `url: Option<String>,`

```
/// Optional explicit URL — when present, fetched verbatim instead of
/// `https://{UPDATE_HOST}{UPDATE_BASE}/assets/<remote_filename>`. The
/// `.sig` sidecar URL is derived by appending `.sig` to this URL.
```

## L133-135 · `pub(super) fn fmt_size(bytes: usize) -> String {`

```
/// Human-readable size, three significant-ish digits, no float formatting.
/// Parenthesised on purpose: the terminal draws `(…)` on a status line in the
/// faint colour, so sizes step back behind names and versions.
```

## L145-146 · `struct Plan {`

```
/// Everything `update` found to do — built without changing a thing, so it
/// can be shown before it is applied.
```

## L166 · `fn current_summary(&self) -> String {`

```
/// "kernel v0.240.0, 18 modules, 14 assets" — everything that needs nothing.
```

## L179 · `super::clear_cancel(); // arm Ctrl+C cancel for this OTA run`

```
// arm Ctrl+C cancel for this OTA run
```

## L190-192 · `let _quiet = (!verbose).then(super::http::quiet);`

```
// Answering "what changed" takes three manifest fetches plus one request
// per item — their connect timings and status lines say nothing about the
// question and buried the answer. `-v` puts them back.
```

## L198-199 · `kprintln!("[npk]   * everything current — {}", plan.current_summary());`

```
// The whole point of the rewrite: nothing to do is ONE line, not one
// line per module and per asset with the four that matter buried in it.
```

## L220-221 · `fn build_plan() -> Option<Plan> {`

```
/// Fetch the three manifests and diff them against what is installed.
/// Reads only — nothing here writes to the ESP or npkFS.
```

## L236-238 · `let kernel = if manifest.version == current {`

```
// The manifest is not authenticated yet — the SHA-384 and signature
// checks in `apply_kernel` are what make it trustworthy. Here it may only
// *lower* our appetite, never raise it past MAX_KERNEL_SIZE.
```

## L270-271 · `let rest = plan.current_summary();`

```
// One line for everything that needs nothing — this used to be one line
// per module and per asset, which buried the few that mattered.
```

## L297-300 · `if certs_changed {`

```
// New anchors are inert until reloaded — the store is held in memory so
// handshakes never touch npkFS. Without this, a freshly delivered CA
// would only take effect after the next reboot, which looks exactly
// like the update not having worked.
```

## L316 · `fn apply_kernel(manifest: &Manifest) -> bool {`

```
/// Download, verify and write the kernel to the ESP.
```

## L375-376 · `}`

```
// Section header without all fields — discard whatever partial
// state we collected so it doesn't leak into the next entry.
```

## L402-403 · `fn split_url(url: &str) -> Option<(&str, &str)> {`

```
/// Split a full `https://host/path` URL into (host, path). Used to feed
/// `https_get` (which expects them separate) from a manifest `url=` line.
```

## L412 · `struct AssetJob {`

```
/// One asset the release has a different build of, resolved to its paths.
```

## L417 · `present: bool,`

```
/// Whether a copy already exists locally (only differs in wording).
```

## L421-422 · `fn plan_assets() -> (Vec<AssetJob>, usize) {`

```
/// Diff release/assets/manifest against npkFS-resident assets. Reads only:
/// returns the jobs to run and how many were already current.
```

## L441-446 · `None => match entry.section.strip_prefix("cert:").filter(|n| safe_asset_name(n)) {`

```
// Root CA anchors are data, not code: the section name IS the
// filename, so shipping or replacing an anchor is dropping a file
// into `release/assets/certs/` — no kernel change, no reinstall.
// The trust chain is unchanged: size and sha384 come from the
// manifest and every asset is checked against its own detached
// signature on apply, exactly like the kernel.
```

## L474-475 · `fn apply_asset(job: &AssetJob) -> bool {`

```
/// Download, verify and store one planned asset. Prints its own result line;
/// returns whether the asset was written.
```

## L481-489 · `const BLOCK: u64 = 4096;`

```
// ── Make room before a streaming write ──────────────────────
// The streaming writer keeps the OLD copy live until finish(),
// so a refresh transiently needs the new asset's size ON TOP of
// everything already stored — 2× for a same-path replace. A
// previously-aborted download also leaks orphaned chunks that
// only gc reclaims. On a tight partition a 261 MB bundle can't
// afford either, which is why a fresh fetch failed ~40 MB in
// with a bare "npkfs write failed" (= DiskFull). Reclaim and
// free up front; bail with a clear message if it still won't fit.
```

## L494 · `if free_bytes() < need {`

```
// 1. Reclaim orphans from any earlier aborted streaming download.
```

## L502-505 · `if free_bytes() < need && local_present {`

```
// 2. Still short and an old copy exists → drop it first so we
//    don't need 2× the asset size. We're replacing it anyway;
//    on a disk this tight, keeping both isn't an option. The
//    path unlink orphans the chunk blobs; gc reclaims them.
```

## L512 · `if free_bytes() < need {`

```
// 3. Truly out of space — fail clearly instead of 40 MB in.
```

## L521-527 · `let (asset_host, asset_path_owned);`

```
// Two URL paths:
//   (a) entry.url == Some(url)  → fetch verbatim (GitHub Releases).
//       `https_get_streaming` follows 302 redirects so
//       `github.com/.../releases/download/...` → the signed
//       `objects.githubusercontent.com` CDN URL works transparently.
//   (b) entry.url == None       → fall back to raw.githubusercontent
//       on main (the existing flow for <30 MB assets).
```

## L540-543 · `let mut writer = match crate::npkfs::open_streaming_write(spec.npkfs_path) {`

```
// Streaming download: drive bytes straight into npkFS via the
// ChunkedWriter, hashing SHA-384 incrementally as they pass.
// Peak RAM = one 16 MiB chunk regardless of asset size — a
// 1 GB userspace bundle no longer needs a 1 GB heap spike.
```

## L551-553 · `const STEP: usize = 8 * 1024 * 1024;`

```
// Progress heartbeat every 8 MiB. The asset size is known
// from the manifest so we can show a percentage — the
// download blocks the shell, the line proves it's alive.
```

## L587-590 · `drop(writer);`

```
// Drop the partial writer and sweep the chunks it already
// flushed — they're unreachable (finish() never ran), so a
// retry starts from a clean slate instead of accumulating
// orphaned space across attempts.
```

## L604-606 · `return false;`

```
// Drop the writer without finishing — flushed chunks
// remain in storage but are unreachable from the path
// tree, so the next `gc()` cycle reclaims them.
```

## L610 · `let sig_host_owned;`

```
// Sig URL: `<asset_url>.sig` if url= override, else default path.
```

## L637-638 · `return false;`

```
// Writer is dropped without finish; chunks become
// unreachable, gc reclaims them on next pass.
```

## L642-643 · `match writer.finish() {`

```
// Commit: writer.finish() publishes the chunked file
// atomically. Replaces any existing entry at the same path.
```

## L653-656 · `fn safe_asset_name(name: &str) -> bool {`

```
/// A manifest-supplied asset name we are willing to turn into a path.
/// Deliberately strict — the name becomes part of an npkFS path, so anything
/// that could climb out of its directory is refused. The manifest is
/// signature-checked per asset, but a name is not a place to be trusting.
```

## L664-665 · `struct AssetRef<'a> {`

```
/// One asset's two paths, borrowed — the static table and the dynamic
/// cert case produce the same shape.
```

