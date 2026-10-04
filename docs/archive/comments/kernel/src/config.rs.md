# `kernel/src/config.rs` @ 5e0102684

## L1-6 · `use alloc::string::String;`

```
//! System Configuration
//!
//! Key-value config stored at ".system/config" in npkFS (encrypted at
//! rest). Loaded into memory after unlock, persisted on every change.
//!
//! Format: "key=value\n" lines, UTF-8.
```

## L14 · `pub const CONFIG_OBJECT: &str = ".system/config";`

```
/// Path of the encrypted config blob (was `.npk-config` in v1).
```

## L17 · `pub const KEYCHECK_PATH: &str = ".system/keycheck";`

```
/// Path of the passphrase verifier blob (was `.npk-keycheck` in v1).
```

## L20-21 · `pub const KEYCHECK_VALUE: &[u8] = b"nopeekOS.keycheck.v1.valid";`

```
/// Magic bytes written into KEYCHECK_PATH at install time. Decryption
/// of the blob produces these bytes iff the master key is correct.
```

## L98 · `pub fn load() {`

```
/// Load config from npkFS. Call after unlock.
```

## L105 · `}`

```
// No config yet — fresh system
```

## L110 · `fn save() {`

```
/// Persist config to npkFS.
```

## L116 · `pub fn get(key: &str) -> Option<String> {`

```
/// Get a config value.
```

## L121 · `pub fn set(key: &str, value: &str) {`

```
/// Set a config value and persist.
```

## L127 · `pub fn unset(key: &str) -> bool {`

```
/// Remove a config value and persist.
```

## L134 · `pub fn timezone_offset_minutes() -> i32 {`

```
/// Get timezone offset in minutes (e.g. +120 for UTC+2).
```

## L143 · `fn parse_timezone_offset(s: &str) -> i32 {`

```
/// Parse timezone string: "+2", "-5", "+5:30", "+05:45"
```

## L166 · `pub fn list() -> Vec<(String, String)> {`

```
/// List all config entries.
```

## L173 · `pub const KNOWN_KEYS: &[(&str, &str)] = &[`

```
/// Known config keys with descriptions.
```

## L185-186 · `("net.allow_plain_http",`

```
// Deliberately last, and deliberately spelled out: this one lowers a
// security guarantee. Everything else here is taste or topology.
```

