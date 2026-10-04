# `kernel/src/intent/auth.rs` @ 5e0102684

## L1 · `use crate::{kprint, kprintln, crypto, serial};`

```
//! Authentication intents: lock, passwd
```

## L11 · `if crate::framebuffer::is_available() {`

```
// Use GUI login screen if framebuffer available
```

## L15 · `let mut attempts: u32 = 0;`

```
// Fallback: text-mode unlock
```

## L65 · `kprint!("[npk] Current passphrase: ");`

```
// Verify current passphrase
```

## L77 · `let saved_key = crypto::get_master_key();`

```
// Temporarily set old key to verify
```

## L84 · `if let Some(k) = saved_key { crypto::set_master_key(k); }`

```
// Restore original key
```

## L91 · `let _ = crate::npkfs::delete(crate::config::KEYCHECK_PATH);`

```
// Delete old keycheck (still encrypted with old key)
```

## L94 · `let new_key = loop {`

```
// Get new passphrase
```

## L121 · `crypto::set_master_key(new_key);`

```
// Set new key and re-encrypt keycheck
```

