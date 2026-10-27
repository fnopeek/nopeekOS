//! A media file read range by range, so a film never has to fit in memory.
//!
//! Reads go through a window: the samples of one stream lie close together,
//! so one host call fills the window and the next samples come out of it.
//! Each stream holds its own `Media`, because audio and video advance
//! through different parts of the file.

use alloc::vec::Vec;

use crate::host::File;

/// Bytes one refill reads ahead. A sample larger than this is read whole.
const WINDOW: usize = 1024 * 1024;

pub struct Media {
    file: File,
    /// Bytes from `at` on, as last read.
    win: Vec<u8>,
    at: u64,
}

impl Media {
    pub fn open(path: &str) -> Option<Media> {
        Some(Media { file: File::open(path)?, win: Vec::new(), at: 0 })
    }

    pub fn size(&self) -> u64 { self.file.size() }

    /// Fill `buf` from `offset`, past the window; false if the file ends first.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> bool {
        self.file.read_exact_at(offset, buf)
    }

    /// The `len` bytes at `offset`, or `None` if they do not lie in the file
    /// or cannot be read.
    pub fn get(&mut self, offset: u64, len: usize) -> Option<&[u8]> {
        let end = offset.checked_add(len as u64)?;
        if end > self.size() { return None; }
        let have = self.at.checked_add(self.win.len() as u64)?;
        if offset < self.at || end > have {
            let want = len.max(WINDOW).min(usize::try_from(self.size() - offset).unwrap_or(usize::MAX));
            self.win.clear();
            // A table that names a huge sample is refused, not an abort.
            self.win.try_reserve(want).ok()?;
            self.win.resize(want, 0);
            self.at = offset;
            if !self.file.read_exact_at(offset, &mut self.win) {
                self.win.clear();
                return None;
            }
        }
        let from = usize::try_from(offset - self.at).ok()?;
        self.win.get(from..from.checked_add(len)?)
    }
}
