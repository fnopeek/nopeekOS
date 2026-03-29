//! Cryptographically Secure PRNG (ChaCha20-based)
//!
//! Replaces xorshift128+ for capability tokens and all security-sensitive randomness.
//! Seeded from RDRAND (hardware RNG) if available, TSC fallback.
//! Re-keys every 64 blocks for forward secrecy.

use spin::Mutex;
use crate::kprintln;

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

    fn next_u128(&mut self) -> u128 {
        let hi = self.next_u64() as u128;
        let lo = self.next_u64() as u128;
        (hi << 64) | lo
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

fn build_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];

    if has_rdrand() {
        // Hardware RNG: best entropy source
        for i in 0..4 {
            // Retry up to 10 times per word
            for _ in 0..10 {
                if let Some(val) = rdrand64() {
                    seed[i * 8..(i + 1) * 8].copy_from_slice(&val.to_le_bytes());
                    break;
                }
            }
        }
    } else {
        // Fallback: TSC + constants (NOT ideal but better than nothing)
        let t1 = rdtsc();
        let t2 = rdtsc();
        let t3 = rdtsc();
        let t4 = rdtsc();
        seed[0..8].copy_from_slice(&(t1 ^ 0x6A09E667F3BCC908).to_le_bytes());
        seed[8..16].copy_from_slice(&(t2 ^ 0xBB67AE8584CAA73B).to_le_bytes());
        seed[16..24].copy_from_slice(&(t3 ^ 0x3C6EF372FE94F82B).to_le_bytes());
        seed[24..32].copy_from_slice(&(t4 ^ 0xA54FF53A5F1D36F1).to_le_bytes());
    }

    seed
}

// === Public API ===

pub fn init() {
    let seed = build_seed();
    *RNG.lock() = Some(ChaChaRng::new(&seed));

    if has_rdrand() {
        kprintln!("[npk] CSPRNG: ChaCha20 (RDRAND-seeded)");
    } else {
        kprintln!("[npk] CSPRNG: ChaCha20 (TSC-seeded, no RDRAND)");
    }
}

pub fn random_u128() -> u128 {
    let mut rng = RNG.lock();
    let rng = rng.as_mut().expect("CSPRNG not initialized");
    loop {
        let val = rng.next_u128();
        if val != 0 { return val; }
    }
}

pub fn random_u64() -> u64 {
    let mut rng = RNG.lock();
    let rng = rng.as_mut().expect("CSPRNG not initialized");
    rng.next_u64()
}
