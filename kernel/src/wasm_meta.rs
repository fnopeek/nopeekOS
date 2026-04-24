//! Pull a named custom section out of a raw WASM binary. Used by
//! `install.rs` to extract `.npk.app_meta` without spinning up wasmi.

pub fn extract_custom_section<'a>(wasm: &'a [u8], target_name: &str) -> Option<&'a [u8]> {
    if wasm.len() < 8 { return None; }
    if &wasm[0..4] != b"\0asm" { return None; }
    if &wasm[4..8] != &[0x01, 0x00, 0x00, 0x00] { return None; }

    let mut cur = &wasm[8..];
    while !cur.is_empty() {
        let section_id = cur[0];
        cur = &cur[1..];
        let (section_size, consumed) = read_leb128_u32(cur)?;
        cur = &cur[consumed..];
        if section_size as usize > cur.len() { return None; }
        let (payload, rest) = cur.split_at(section_size as usize);
        cur = rest;

        if section_id != 0 { continue; }

        let (name_len, consumed) = match read_leb128_u32(payload) {
            Some(p) => p,
            None    => continue,
        };
        let name_end = consumed + name_len as usize;
        if name_end > payload.len() { continue; }

        if &payload[consumed..name_end] == target_name.as_bytes() {
            return Some(&payload[name_end..]);
        }
    }

    None
}

fn read_leb128_u32(buf: &[u8]) -> Option<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 32 { return None; }
        let payload = (b & 0x7F) as u32;
        if shift == 28 && (payload & !0x0F) != 0 { return None; }
        result |= payload << shift;
        if (b & 0x80) == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_wasm_with_custom(name: &str, data: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec::Vec::new();
        out.extend_from_slice(b"\0asm");
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);

        let mut body = alloc::vec::Vec::new();
        write_leb128_u32(&mut body, name.len() as u32);
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(data);

        out.push(0);
        write_leb128_u32(&mut out, body.len() as u32);
        out.extend_from_slice(&body);
        out
    }

    fn write_leb128_u32(dst: &mut alloc::vec::Vec<u8>, mut v: u32) {
        loop {
            let mut byte = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 { byte |= 0x80; }
            dst.push(byte);
            if v == 0 { break; }
        }
    }

    #[test]
    fn finds_matching_section() {
        let wasm = make_wasm_with_custom(".npk.app_meta", &[1, 2, 3, 4, 5]);
        assert_eq!(extract_custom_section(&wasm, ".npk.app_meta"), Some(&[1, 2, 3, 4, 5][..]));
    }

    #[test]
    fn returns_none_for_missing_section() {
        let wasm = make_wasm_with_custom("some-other", &[0]);
        assert!(extract_custom_section(&wasm, ".npk.app_meta").is_none());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut wasm = make_wasm_with_custom(".npk.app_meta", &[0]);
        wasm[0] = b'X';
        assert!(extract_custom_section(&wasm, ".npk.app_meta").is_none());
    }

    #[test]
    fn rejects_wrong_version() {
        let mut wasm = make_wasm_with_custom(".npk.app_meta", &[0]);
        wasm[4] = 0x02;
        assert!(extract_custom_section(&wasm, ".npk.app_meta").is_none());
    }
}
