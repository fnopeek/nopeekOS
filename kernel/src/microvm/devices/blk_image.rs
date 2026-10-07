//! Backing store of a virtio-blk disk. An image stored in npkFS as a chunked
//! object is read one chunk at a time, the first time the guest touches it;
//! anything else is held whole in RAM. Saving writes only the chunks the
//! guest changed and reuses the stored ones for the rest.

use alloc::boxed::Box;
use alloc::vec::Vec;

pub struct BlkImage {
    len: usize,
    /// Bytes per chunk; every chunk but the last has exactly this size.
    chunk: usize,
    /// Storage hash per chunk, or empty when the image did not come from a
    /// chunked object (then every chunk is resident from the start).
    hashes: Vec<[u8; 32]>,
    data: Vec<Option<Box<[u8]>>>,
    dirty: Vec<bool>,
    /// Set when a chunk failed to load; reads and writes then fail with
    /// IOERR instead of handing the guest zeros.
    failed: bool,
    loaded: usize,
}

impl BlkImage {
    /// A resident image built from `bytes`.
    pub fn resident(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            len,
            chunk: len.max(1),
            hashes: Vec::new(),
            data: alloc::vec![Some(bytes.into_boxed_slice())],
            dirty: alloc::vec![true],
            failed: false,
            loaded: 1,
        }
    }

    /// Open `path` for chunk-wise reads. `None` if it is missing, stored as
    /// one Blob, or its chunks are not uniformly sized; the caller then
    /// falls back to a whole read.
    pub fn open_lazy(path: &str) -> Option<Self> {
        let (total, hashes) = crate::npkfs::chunk_list(path).ok()??;
        let len = usize::try_from(total).ok()?;
        let first = crate::npkfs::read_chunk(hashes.first()?).ok()?;
        let chunk = first.len();
        if chunk == 0 || hashes.len() != len.div_ceil(chunk) {
            return None;
        }
        let n = hashes.len();
        let mut data: Vec<Option<Box<[u8]>>> = (0..n).map(|_| None).collect();
        data[0] = Some(first.into_boxed_slice());
        Some(Self {
            len,
            chunk,
            hashes,
            data,
            dirty: alloc::vec![false; n],
            failed: false,
            loaded: 1,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Chunks read from npkFS so far, out of all chunks.
    pub fn residency(&self) -> (usize, usize) {
        (self.loaded, self.data.len())
    }

    fn ensure(&mut self, i: usize) -> bool {
        if self.data[i].is_some() {
            return true;
        }
        if self.failed {
            return false;
        }
        match crate::npkfs::read_chunk(&self.hashes[i]) {
            Ok(c) if c.len() == self.chunk_len(i) => {
                self.data[i] = Some(c.into_boxed_slice());
                self.loaded += 1;
                true
            }
            _ => {
                crate::kprintln!("[virtio-blk] chunk {} unreadable — disk fails from here", i);
                self.failed = true;
                false
            }
        }
    }

    fn chunk_len(&self, i: usize) -> usize {
        (self.len - i * self.chunk).min(self.chunk)
    }

    /// Bytes at `off`, at most `max` long and never across a chunk
    /// boundary. `None` past the end or on a read failure.
    pub fn read_at(&mut self, off: usize, max: usize) -> Option<&[u8]> {
        if off >= self.len {
            return None;
        }
        let i = off / self.chunk;
        if !self.ensure(i) {
            return None;
        }
        let at = off - i * self.chunk;
        let n = max.min(self.chunk_len(i) - at);
        self.data[i].as_deref().map(|c| &c[at..at + n])
    }

    /// Writable bytes at `off`, like [`Self::read_at`]; marks the chunk dirty.
    pub fn write_at(&mut self, off: usize, max: usize) -> Option<&mut [u8]> {
        if off >= self.len {
            return None;
        }
        let i = off / self.chunk;
        if !self.ensure(i) {
            return None;
        }
        self.dirty[i] = true;
        let at = off - i * self.chunk;
        let n = max.min(self.chunk_len(i) - at);
        self.data[i].as_deref_mut().map(|c| &mut c[at..at + n])
    }

    /// Write the image to `path`. Clean chunks keep their stored blob when
    /// the chunk size matches the writer's; everything else is written out.
    /// Returns `(chunks written, chunks reused)`.
    pub fn save(&mut self, path: &str) -> Result<(usize, usize), crate::npkfs::FsError> {
        if self.failed {
            return Err(crate::npkfs::FsError::Corrupt);
        }
        let reuse = !self.hashes.is_empty() && self.chunk == crate::npkfs::STREAMING_CHUNK_SIZE;
        let mut w = crate::npkfs::open_streaming_write(path)?;
        let (mut written, mut reused) = (0, 0);
        for i in 0..self.data.len() {
            if reuse && !self.dirty[i] {
                w.append_stored_chunk(self.hashes[i], self.chunk_len(i))
                    .map_err(|_| crate::npkfs::FsError::Corrupt)?;
                reused += 1;
                continue;
            }
            if !self.ensure(i) {
                return Err(crate::npkfs::FsError::Corrupt);
            }
            let c = self.data[i].as_deref().unwrap_or(&[]);
            w.write(c).map_err(|_| crate::npkfs::FsError::Corrupt)?;
            written += 1;
        }
        w.finish().map_err(|_| crate::npkfs::FsError::Corrupt)?;
        Ok((written, reused))
    }
}
