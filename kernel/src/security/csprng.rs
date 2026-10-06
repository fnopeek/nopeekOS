//! Cryptographically Secure PRNG (ChaCha20-based)
//!
//! Source for capability tokens, the disk data key and all other
//! security-sensitive randomness. The seed is BLAKE3 over hardware RNG words
//! (RDSEED, else RDRAND) that pass a health check, TSC jitter and the RTC, so
//! a broken hardware source cannot make the seed a constant on its own.
//! Re-keys every 64 blocks for forward secrecy; `reseed` mixes in fresh
//! material before a long-lived key is drawn.

use spin::Mutex;

static RNG: Mutex<Option<ChaChaRng>> = Mutex::new(None);

struct ChaChaRng {
    key: [u8; 32],
    counter: u32,
    buffer: [u8; 64],
    pos: usize,
}

impl ChaChaRng {
    fn new(seed: &[u8; 32]) -> Self {
        let mut rng = ChaChaRng { key: *seed, counter: 0, buffer: [0; 64], pos: 64 };
        rng.refill();
        // Discard first block (defense against weak seeds)
        rng.refill();
        rng
    }

    fn refill(&mut self) {
        self.buffer = chacha20_block(&self.key, self.counter, &[0u8; 12]);
        self.counter = self.counter.wrapping_add(1);
        self.pos = 0;

        // Re-key every 64 blocks for forward secrecy
        if self.counter % 64 == 0 {
            let mut new_key = [0u8; 32];
            new_key.copy_from_slice(&self.buffer[..32]);
            self.key = new_key;
            self.buffer = chacha20_block(&self.key, self.counter, &[0u8; 12]);
            self.counter = self.counter.wrapping_add(1);
        }
    }

    fn next_u64(&mut self) -> u64 {
        if self.pos + 8 > 64 { self.refill(); }
        let val = u64::from_le_bytes(self.buffer[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        val
    }

    #[allow(dead_code)]
    fn next_u128(&mut self) -> u128 {
        let hi = self.next_u64() as u128;
        let lo = self.next_u64() as u128;
        (hi << 64) | lo
    }

    /// Fill `out` from the keystream, block-wise from the same buffer as
    /// `next_u64`. `crypto.getRandomValues` may ask for up to 65536 bytes,
    /// so this avoids going through 64-bit words.
    fn fill(&mut self, out: &mut [u8]) {
        let mut done = 0;
        while done < out.len() {
            if self.pos >= 64 { self.refill(); }
            let take = (64 - self.pos).min(out.len() - done);
            out[done..done + take].copy_from_slice(&self.buffer[self.pos..self.pos + take]);
            self.pos += take;
            done += take;
        }
    }

    fn next_256(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..4 {
            let val = self.next_u64();
            out[i * 8..(i + 1) * 8].copy_from_slice(&val.to_le_bytes());
        }
        out
    }
}

// === ChaCha20 core (RFC 7539) ===

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut s = [0u32; 16];

    // "expand 32-byte k"
    s[0] = 0x61707865; s[1] = 0x3320646e;
    s[2] = 0x79622d32; s[3] = 0x6b206574;

    for i in 0..8 {
        let o = i * 4;
        s[4 + i] = u32::from_le_bytes([key[o], key[o + 1], key[o + 2], key[o + 3]]);
    }
    s[12] = counter;
    for i in 0..3 {
        let o = i * 4;
        s[13 + i] = u32::from_le_bytes([nonce[o], nonce[o + 1], nonce[o + 2], nonce[o + 3]]);
    }

    let initial = s;

    for _ in 0..10 {
        // Column rounds
        quarter_round(&mut s, 0, 4,  8, 12);
        quarter_round(&mut s, 1, 5,  9, 13);
        quarter_round(&mut s, 2, 6, 10, 14);
        quarter_round(&mut s, 3, 7, 11, 15);
        // Diagonal rounds
        quarter_round(&mut s, 0, 5, 10, 15);
        quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7,  8, 13);
        quarter_round(&mut s, 3, 4,  9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let val = s[i].wrapping_add(initial[i]);
        out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
    }
    out
}

// === Seeding ===

fn has_rdrand() -> bool {
    let ecx: u32;
    // SAFETY: CPUID is always available on x86_64. rbx is saved/restored
    // because LLVM uses it internally.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    ecx & (1 << 30) != 0
}

fn rdrand64() -> Option<u64> {
    let val: u64;
    let ok: u8;
    // SAFETY: RDRAND is available (checked by has_rdrand)
    unsafe {
        core::arch::asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
        );
    }
    if ok == 1 { Some(val) } else { None }
}

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32) | (lo as u64)
}

fn has_rdseed() -> bool {
    let ebx: u32;
    // SAFETY: CPUID leaf 7 subleaf 0 exists on every x86_64 we boot on; rbx
    // is saved by hand because LLVM reserves it.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => _,
            out("edx") _,
        );
    }
    ebx & (1 << 18) != 0
}

fn rdseed64() -> Option<u64> {
    let val: u64;
    let ok: u8;
    // SAFETY: RDSEED is available (checked by has_rdseed).
    unsafe {
        core::arch::asm!(
            "rdseed {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
        );
    }
    if ok == 1 { Some(val) } else { None }
}

/// Words drawn from the hardware RNG for one seed.
const HW_WORDS: usize = 8;

/// Hardware RNG words, or None when the source is missing or fails the
/// health check. RDSEED (an entropy source) is preferred; RDRAND (a DRBG
/// seeded from it) is the fallback. Rejected, as Linux does at boot: a word
/// that is all zeros or all ones (the AMD erratum returns all ones with CF
/// set), and a sample in which any word repeats.
fn hardware_words() -> Option<[u64; HW_WORDS]> {
    let source: fn() -> Option<u64> = if has_rdseed() {
        rdseed64
    } else if has_rdrand() {
        rdrand64
    } else {
        return None;
    };
    let mut words = [0u64; HW_WORDS];
    for w in words.iter_mut() {
        // RDSEED may run dry for a moment; give it time before giving up.
        let mut got = None;
        for _ in 0..100 {
            if let Some(v) = source() {
                got = Some(v);
                break;
            }
            core::hint::spin_loop();
        }
        let v = got?;
        if v == 0 || v == u64::MAX {
            return None;
        }
        *w = v;
    }
    for i in 0..HW_WORDS {
        for j in i + 1..HW_WORDS {
            if words[i] == words[j] {
                return None;
            }
        }
    }
    Some(words)
}

/// Timing jitter: TSC deltas around memory traffic. A few bits each, but
/// independent of the hardware RNG.
fn jitter(h: &mut blake3::Hasher) {
    let mut scratch = [0u64; 64];
    let mut prev = rdtsc();
    for i in 0..1024usize {
        let slot = (prev as usize ^ i.wrapping_mul(0x9E37_79B9)) % scratch.len();
        scratch[slot] = scratch[slot].wrapping_add(prev).rotate_left(7);
        let now = rdtsc();
        h.update(&now.wrapping_sub(prev).to_le_bytes());
        prev = now;
    }
    h.update(&scratch[0].to_le_bytes());
}

/// Fresh seed material, and whether a hardware source passed.
fn build_seed() -> ([u8; 32], bool) {
    let mut h = blake3::Hasher::new_derive_key("nopeekOS csprng seed v1");
    let hw = hardware_words();
    if let Some(words) = hw {
        for w in words {
            h.update(&w.to_le_bytes());
        }
    }
    jitter(&mut h);
    if let Some(t) = crate::rtc::read_unix_time() {
        h.update(&t.to_le_bytes());
    }
    (*h.finalize().as_bytes(), hw.is_some())
}

static HW_SEEDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether the last seed or reseed had a hardware source that passed.
pub fn hardware_seeded() -> bool {
    HW_SEEDED.load(core::sync::atomic::Ordering::Relaxed)
}

fn report(hw: bool) {
    HW_SEEDED.store(hw, core::sync::atomic::Ordering::Relaxed);
    if !hw {
        crate::kprintln!("[npk] CSPRNG: WARNING — no hardware RNG passed its check; \
            seeded from timing jitter only");
    }
}

// === Public API ===

pub fn init() {
    let (seed, hw) = build_seed();
    *RNG.lock() = Some(ChaChaRng::new(&seed));
    report(hw);
    if hw {
        let src = if has_rdseed() { "RDSEED" } else { "RDRAND" };
        crate::kdebug!("[npk] CSPRNG: ready ({}-seeded)", src);
    }
}

/// Mix fresh hardware words, jitter and the RTC into the generator. Called
/// before a long-lived key is drawn, so it does not rest on the boot seed
/// alone.
pub fn reseed() {
    let (fresh, hw) = build_seed();
    {
        let mut guard = RNG.lock();
        let Some(rng) = guard.as_mut() else { return };
        let mut h = blake3::Hasher::new_derive_key("nopeekOS csprng reseed v1");
        h.update(&rng.key);
        h.update(&fresh);
        rng.key = *h.finalize().as_bytes();
        rng.refill();
    }
    report(hw);
}

#[allow(dead_code)]
pub fn random_u128() -> u128 {
    let mut rng = RNG.lock();
    let rng = rng.as_mut().expect("CSPRNG not initialized");
    loop {
        let val = rng.next_u128();
        if val != 0 { return val; }
    }
}

/// Generate a 256-bit random value for capability tokens (post-quantum safe).
pub fn random_256() -> [u8; 32] {
    let mut rng = RNG.lock();
    let rng = rng.as_mut().expect("CSPRNG not initialized");
    loop {
        let val = rng.next_256();
        if val != [0u8; 32] { return val; }
    }
}

/// Fill a buffer with random bytes; the source behind a page's
/// `crypto.getRandomValues`. Deliberately the same ChaCha20 stream as the
/// capability tokens: pages build session tokens from it, so a second,
/// weaker source would be a trap.
pub fn fill(out: &mut [u8]) {
    let mut rng = RNG.lock();
    let rng = rng.as_mut().expect("CSPRNG not initialized");
    rng.fill(out);
}

#[allow(dead_code)]
pub fn random_u64() -> u64 {
    let mut rng = RNG.lock();
    let rng = rng.as_mut().expect("CSPRNG not initialized");
    rng.next_u64()
}
