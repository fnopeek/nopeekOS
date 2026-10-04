# `kernel/src/security/capability.rs` @ 5e0102684

## L1-5 · `use bitflags::bitflags;`

```
//! Capability System
//!
//! Security foundation of nopeekOS. No chmod, no ACLs, no users.
//! Every permission is a token with a random 256-bit ID (post-quantum safe).
//! Inspired by seL4 capabilities.
```

## L12 · `pub type CapId = [u8; 32];`

```
/// 256-bit capability token ID (post-quantum: Grover-safe at 128-bit effective)
```

## L15 · `pub const CAP_NULL: CapId = [0u8; 32];`

```
/// Null capability (no access)
```

## L32 · `const RENDER    = 0b0100_0000;`

```
/// Phase 10: `npk_scene_commit` — widget-tree rendering.
```

## L34-35 · `const CANVAS    = 0b1000_0000;`

```
/// P10.10: `npk_canvas_commit` — upload a raw BGRA bitmap into a
/// `Widget::Canvas` (image viewer, future paint app).
```

## L37-38 · `const CAPTURE   = 0b1_0000_0000;`

```
/// `npk_capture_screen` — read the composited framebuffer
/// (screenshot tool). Highly privileged: screen-scrape.
```

## L40-42 · `const HARDWARE  = 0b10_0000_0000;`

```
/// `npk_acpi_dsdt` / `npk_ec_read` / `npk_ec_write` — raw firmware +
/// embedded-controller access (the AML battery driver). Highly
/// privileged: direct hardware I/O.
```

## L44-46 · `const NETCTL    = 0b100_0000_0000;`

```
/// `npk_wifi_send_cmd` / `npk_wifi_poll_event` — the WiFi-class control
/// channel manager side (wifid.wasm supplicant). Privileged: can scan,
/// connect, and read EAPOL handshake frames. See docs/spec/WIFI_CLASS_ABI.md.
```

## L48-50 · `const NET       = 0b1000_0000_0000;`

```
/// `npk_http_request` — outbound HTTPS fetch for the native browser
/// (beak). Distinct from NETCTL (WiFi-supplicant control): NET is
/// simply "may make outbound TLS requests", the browser's fetch cap.
```

## L77 · `pub expires_at: Option<u64>,`

```
/// Tick at which this capability expires. None = no expiry.
```

## L97-99 · `pub fn create(`

```
/// Create a new capability delegated from parent.
/// Rights monotonicity: delegated rights ⊆ parent rights.
/// Temporal monotonicity: child expiry ≤ parent expiry.
```

## L131 · `let expires_at = match (ttl_ticks.map(|ttl| interrupts::ticks() + ttl), parent.expires_at) {`

```
// Child expiry cannot exceed parent expiry
```

## L151-152 · `pub fn init() -> (&'static Mutex<Vault>, CapId) {`

```
/// Initialize vault with root capability. Returns vault ref + root cap ID.
/// Requires csprng::init() to be called first.
```

## L171 · `#[allow(dead_code)]`

```
/// Revoke a capability and all its children (transitive)
```

## L184 · `pub fn check(&self, cap_id: &CapId, required: Rights) -> Result<&Capability, CapError> {`

```
/// Check if a capability grants the required rights
```

## L232 · `pub fn create_module_cap(rights: Rights, ttl_ticks: Option<u64>) -> Result<CapId, CapError> {`

```
/// Create a capability for a WASM module (delegates from root internally).
```

## L239-256 · `const MAX_PATH_GRANTS: usize = 64;`

```
// ── Per-path grants ───────────────────────────────────────────────────
//
// The file dialog's whole point: a user picking a file in trusted UI is
// the authorisation, so the app that asked never needs blanket WRITE.
// `npk_pick` records a grant for exactly the chosen path against the
// requester's capability; `npk_store` accepts it in place of the global
// right.
//
// Narrow on purpose:
//   - one exact path, no prefix matching, no directories
//   - bound to the capability of the instance that asked, so it dies
//     with the app
//   - never covers `sys/wasm/` — that check runs before this one and a
//     grant must not be a way around it
//
// Bounded array rather than a growing map: a runaway app must not be
// able to make the kernel allocate. Oldest entry is evicted when full,
// which at worst costs a user one re-pick.
```

## L269-270 · `pub fn grant_path(cap: CapId, path: &str, rights: Rights) {`

```
/// Record that `cap` may act on `path` with `rights`. Re-granting the
/// same pair widens the existing entry rather than stacking duplicates.
```

## L281 · `pub fn check_path_grant(cap: &CapId, path: &str, rights: Rights) -> bool {`

```
/// True if `cap` holds a grant covering exactly `path` with `rights`.
```

## L287-288 · `pub fn revoke_path_grants(cap: &CapId) {`

```
/// Drop every grant held by `cap` — called when an instance ends, so a
/// recycled capability id can never inherit someone else's file.
```

## L293-300 · `const CAP_BIT_READ:    u8 = 0x01;`

```
// ── Per-app capability declaration (`.npk.caps` section) ──────────────
//
// A widget app self-declares the rights it needs in a 1-byte custom
// section. The spawn path (npk_spawn_module / launch_app / spawn_launcher)
// reads it and grants EXACTLY those rights — no blanket WRITE. The bit
// layout mirrors `nopeek_widgets::caps` in the SDK. An absent or
// malformed section falls back to a safe default that never includes
// WRITE, so a future app cannot silently gain write access.
```

## L324-326 · `const CAP2_BIT_NET: u8 = 0x01; // npk_http_request (beak's outbound fetch)`

```
// Second `.npk.caps` byte — the first byte's 8 bits are full. Apps that
// need an extension right ship a 2-byte section; a 1-byte (or absent)
// section grants nothing from byte 2.
```

## L327 · `const CAP2_BIT_NET: u8 = 0x01; // npk_http_request (beak's outbound fetch)`

```
// npk_http_request (beak's outbound fetch)
```

## L335-337 · `fn default_widget_rights() -> Rights {`

```
/// Rights granted to a widget app that ships no `.npk.caps` section:
/// read + execute + render, but NOT write. Matches the pre-per-app
/// behavior minus the blanket WRITE.
```

## L342-343 · `pub fn widget_rights_from_wasm(wasm: &[u8]) -> Rights {`

```
/// Resolve the rights to grant a widget module from its `.npk.caps`
/// custom section, or the safe default if absent / malformed.
```

## L355-357 · `fn extract_wasm_custom_section<'a>(wasm: &'a [u8], target: &str) -> Option<&'a [u8]> {`

```
/// Minimal wasm custom-section reader (mirrors the SDK's app_catalog
/// parser): walks the module's sections and returns the payload of the
/// first custom section (id 0) whose name matches `target`.
```

## L394 · `pub fn check_global(cap_id: &CapId, required: Rights) -> Result<(), CapError> {`

```
/// Check a capability against the global vault.
```

## L399 · `pub fn create_driver_cap(`

```
/// Create a capability for a WASM driver module bound to a specific PCI device.
```

## L408 · `pub fn check_pci_device(`

```
/// Check that a capability grants access to a specific PCI device.
```

## L422 · `pub fn short_id(id: &CapId) -> u32 {`

```
/// Short hex representation of a 256-bit cap ID (first 8 hex chars = 4 bytes)
```

