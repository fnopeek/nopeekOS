//! gzip on the receive path (RFC 1952), streaming and capped.
//!
//! * Streaming, not buffer-then-unpack: the receive path hands fragments to
//!   a sink; a staging buffer would hold the whole response a second time.
//! * The cap is the same as without gzip: at most `max_size` inflated bytes
//!   pass. A zip bomb is then no worse than an uncompressed body of that
//!   size, and the remote side never decides how much we unpack.
//!
//! miniz_oxide knows zlib and raw DEFLATE, not the gzip framing, so the
//! header is parsed here and the rest is fed as `Raw`.
//!
//! The trailer's CRC32 is not verified: the bytes came through TLS, which
//! already vouches for their integrity.

use alloc::boxed::Box;
use alloc::vec::Vec;
use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

/// A gzip header is variable-length (file name, comment). No real server
/// needs more than this, and without a bound the far side could keep us busy
/// with an endless FNAME field.
const MAX_HEADER: usize = 4096;

/// Inflate buffer per round. Big enough that a 16 KB TLS record clears in a
/// few rounds, small enough for the kernel heap.
const CHUNK: usize = 16 * 1024;

pub struct GzipInflate {
    state: Option<Box<InflateState>>,
    /// Header bytes, while the header is still incomplete.
    hdr: Vec<u8>,
    out: Vec<u8>,
    budget: usize,
    /// Raw bytes fed and inflated bytes produced, for the trace only; they
    /// show that the gzip path actually ran.
    fed: usize,
    produced: usize,
    done: bool,
    /// Report once, not per fragment.
    clipped: bool,
}

impl GzipInflate {
    /// `budget` = how many inflated bytes may pass through the sink at most.
    pub fn new(budget: usize) -> Self {
        GzipInflate {
            state: None,
            hdr: Vec::new(),
            out: alloc::vec![0u8; CHUNK],
            budget,
            fed: 0,
            produced: 0,
            done: false,
            clipped: false,
        }
    }

    /// Raw bytes in, inflated bytes out — the two numbers for the trace.
    pub fn ratio(&self) -> (usize, usize) {
        (self.fed, self.produced)
    }

    pub fn feed(
        &mut self,
        input: &[u8],
        sink: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if self.done {
            return Ok(());
        }
        self.fed += input.len();
        if self.state.is_none() {
            self.hdr.extend_from_slice(input);
            // The bound applies only to a header that does not end. Checking
            // it before the parse would also count the body bytes that
            // arrived in the same fragment.
            let n = match header_len(&self.hdr)? {
                None => {
                    if self.hdr.len() > MAX_HEADER {
                        return Err("gzip header too large");
                    }
                    return Ok(()); // header still incomplete
                }
                Some(n) => n,
            };
            let rest: Vec<u8> = self.hdr[n..].to_vec();
            self.hdr = Vec::new();
            self.state = Some(InflateState::new_boxed(DataFormat::Raw));
            return self.push(&rest, sink);
        }
        self.push(input, sink)
    }

    fn push(
        &mut self,
        mut input: &[u8],
        sink: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return Ok(()),
        };
        // Only while there is input: a call with empty input reports
        // `MZError::Buf` ("no progress possible"), which is not damage.
        while !input.is_empty() {
            let r = inflate(state, input, &mut self.out, MZFlush::None);
            if r.bytes_written > 0 {
                let room = self.budget.saturating_sub(self.produced);
                let take = core::cmp::min(r.bytes_written, room);
                if take > 0 {
                    sink(&self.out[..take])?;
                    self.produced += take;
                }
                if take < r.bytes_written {
                    // Cap reached — exactly as an uncompressed body over
                    // `max_size` gets clipped.
                    if !self.clipped {
                        self.clipped = true;
                        crate::kprintln!(
                            "[npk]   gzip: inflated past {} KiB, clipped",
                            self.budget / 1024
                        );
                    }
                    self.done = true;
                    return Ok(());
                }
            }
            input = &input[r.bytes_consumed..];
            match r.status {
                Ok(MZStatus::StreamEnd) => {
                    self.done = true;
                    return Ok(());
                }
                Err(MZError::Buf) => return Ok(()), // needs more input
                Err(_) => return Err("gzip stream damaged"),
                Ok(_) => {}
            }
            // No progress and nothing more to give: the next delivery
            // continues. Without this guard the loop spins.
            if r.bytes_consumed == 0 && r.bytes_written == 0 {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Length of the gzip header, or `None` while it is still incomplete.
fn header_len(b: &[u8]) -> Result<Option<usize>, &'static str> {
    if b.len() < 10 {
        return Ok(None);
    }
    if b[0] != 0x1f || b[1] != 0x8b {
        return Err("not a gzip stream");
    }
    if b[2] != 8 {
        return Err("unknown gzip method");
    }
    let flg = b[3];
    let mut i = 10usize;
    if flg & 0x04 != 0 {
        // FEXTRA
        if b.len() < i + 2 {
            return Ok(None);
        }
        let xlen = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
        i += 2 + xlen;
    }
    for bit in [0x08u8, 0x10u8] {
        // FNAME, FCOMMENT — one NUL-terminated string each
        if flg & bit != 0 {
            loop {
                if i >= b.len() {
                    return Ok(None);
                }
                let c = b[i];
                i += 1;
                if c == 0 {
                    break;
                }
            }
        }
    }
    if flg & 0x02 != 0 {
        i += 2; // FHCRC
    }
    if b.len() < i {
        return Ok(None);
    }
    Ok(Some(i))
}

/// Inflate a gzip body that is fully in hand.
///
/// The h2 path collects a stream's DATA frames into one Vec anyway — there is
/// nothing to stream there, and the same cap applies.
pub fn inflate_all(body: &[u8], budget: usize) -> Result<Vec<u8>, &'static str> {
    let mut out: Vec<u8> = Vec::new();
    let mut g = GzipInflate::new(budget);
    g.feed(body, &mut |c: &[u8]| {
        out.extend_from_slice(c);
        Ok(())
    })?;
    Ok(out)
}
