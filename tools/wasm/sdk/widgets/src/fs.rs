//! Decoder for the `npk_fs_list` output buffer.
//!
//! The kernel writes one record per entry, records joined by `\n`:
//!
//! ```text
//! <name> \0 <size:u64 LE> \0 <is_dir:u8> \0 <mtime:u64 LE>
//! ```
//!
//! The name is NUL-terminated, so the 19 bytes after it are a fixed-width
//! tail — the record is unambiguous when read **sequentially**. It is not
//! when the buffer is split on `\n` first: `size` and `mtime` are raw
//! little-endian integers and any of their bytes can be `0x0A`. A file of
//! 2600 bytes (`0x0A28`) or a mtime whose low byte happens to be 10 tears
//! its own record in half — the front half is dropped for a short tail,
//! and the back half decodes as a nameless directory carrying garbage.
//! Roughly one entry in sixty. Read records, never lines.

/// One decoded directory entry. Borrows the name out of the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListEntry<'a> {
    pub name:   &'a str,
    pub size:   u64,
    pub is_dir: bool,
    /// UTC seconds since the Unix epoch; zero means unknown.
    pub mtime:  u64,
}

/// `size(8) + sep + is_dir(1) + sep + mtime(8)`.
const TAIL: usize = 19;

/// Decode an `npk_fs_list` buffer. Pass the first `n` bytes the host fn
/// reported written; a non-positive return has no entries to decode.
pub fn list_entries(buf: &[u8]) -> ListIter<'_> {
    ListIter { buf }
}

pub struct ListIter<'a> {
    buf: &'a [u8],
}

impl<'a> Iterator for ListIter<'a> {
    type Item = ListEntry<'a>;

    fn next(&mut self) -> Option<ListEntry<'a>> {
        loop {
            let nul = self.buf.iter().position(|&b| b == 0)?;
            let tail_at = nul + 1;
            // A record that cannot hold its own tail means the buffer is
            // damaged; stop rather than resynchronise on a guess.
            if self.buf.len() < tail_at + TAIL {
                self.buf = &[];
                return None;
            }
            let tail = &self.buf[tail_at..tail_at + TAIL];
            let name = core::str::from_utf8(&self.buf[..nul]);

            let mut next = tail_at + TAIL;
            if self.buf.get(next) == Some(&b'\n') { next += 1; }
            self.buf = &self.buf[next..];

            // A nameless entry is never legitimate, and non-UTF-8 cannot be
            // shown or passed back as a path. Skip and keep going — the
            // records after it are still intact.
            let Ok(name) = name else { continue };
            if name.is_empty() { continue; }

            let mut n8 = [0u8; 8];
            n8.copy_from_slice(&tail[0..8]);
            let size = u64::from_le_bytes(n8);
            n8.copy_from_slice(&tail[11..19]);
            let mtime = u64::from_le_bytes(n8);

            return Some(ListEntry { name, size, is_dir: tail[9] != 0, mtime });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec::Vec;

    fn encode(entries: &[(&str, u64, bool, u64)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(name, size, is_dir, mtime) in entries {
            if !out.is_empty() { out.push(b'\n'); }
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            out.extend_from_slice(&size.to_le_bytes());
            out.push(0);
            out.push(if is_dir { 1 } else { 0 });
            out.push(0);
            out.extend_from_slice(&mtime.to_le_bytes());
        }
        out
    }

    fn names(buf: &[u8]) -> Vec<&str> {
        list_entries(buf).map(|e| e.name).collect()
    }

    #[test]
    fn plain_listing() {
        let buf = encode(&[("a.txt", 196, false, 1786110540), ("sub", 0, true, 0)]);
        let got: Vec<_> = list_entries(&buf).collect();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ListEntry { name: "a.txt", size: 196, is_dir: false, mtime: 1786110540 });
        assert_eq!(got[1], ListEntry { name: "sub", size: 0, is_dir: true, mtime: 0 });
    }

    #[test]
    fn empty_buffer() {
        assert_eq!(names(&[]).len(), 0);
    }

    /// The bug this module exists for: a `0x0A` byte inside `size`.
    #[test]
    fn newline_byte_in_size() {
        for size in [10u64, 2560, 2600, 2815, 68096] {
            let buf = encode(&[
                ("before.txt", 196, false, 1786110540),
                ("victim.md", size, false, 1786110000),
                ("after.txt", 52, false, 1786110600),
            ]);
            assert_eq!(names(&buf), ["before.txt", "victim.md", "after.txt"], "size {size}");
            let victim = list_entries(&buf).nth(1).unwrap();
            assert_eq!(victim.size, size);
            assert_eq!(victim.mtime, 1786110000);
        }
    }

    /// …and one inside `mtime`, which also cost the entry its timestamp.
    #[test]
    fn newline_byte_in_mtime() {
        // 1786110474 has 0x0A as its low byte, 1786055168 as its second.
        for mtime in [1786110474u64, 1786055168, 10] {
            let buf = encode(&[("victim.txt", 196, false, mtime), ("after.txt", 52, false, 1)]);
            assert_eq!(names(&buf), ["victim.txt", "after.txt"], "mtime {mtime}");
            assert_eq!(list_entries(&buf).next().unwrap().mtime, mtime);
        }
    }

    /// A name may legally contain a newline; only the NUL ends it.
    #[test]
    fn newline_in_name() {
        let buf = encode(&[("two\nlines", 5, false, 7), ("next", 1, false, 2)]);
        assert_eq!(names(&buf), ["two\nlines", "next"]);
    }

    #[test]
    fn truncated_tail_stops_cleanly() {
        let mut buf = encode(&[("ok.txt", 1, false, 2), ("cut.txt", 3, false, 4)]);
        buf.truncate(buf.len() - 5);
        assert_eq!(names(&buf), ["ok.txt"]);
    }
}
