//! The device register: what the machine has of each media kind, and the
//! one of each kind that is active. Contract: `docs/spec/DEVICES.md`.
//!
//! Drivers enter what they found (an audio module its outputs, the kernel
//! each USB camera) and take it out again when it goes. The user picks the
//! active one per kind; the pick is kept by id in the config, so a device
//! that comes back is active again. Without a pick, or with the picked one
//! absent, the highest rank wins, the earliest entered among equals.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    AudioOut = 0,
    AudioIn = 1,
    Camera = 2,
}

impl Kind {
    pub fn from_abi(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::AudioOut),
            1 => Some(Self::AudioIn),
            2 => Some(Self::Camera),
            _ => None,
        }
    }

    /// The config key that keeps the user's pick.
    fn key(self) -> &'static str {
        match self {
            Self::AudioOut => "devices.audio_out",
            Self::AudioIn => "devices.audio_in",
            Self::Camera => "devices.camera",
        }
    }
}

/// The default rule's order (`DEVICES.md`): 3 a plugged jack or headset,
/// 2 a USB device, 1 built in, 0 a display's audio, which wins only when
/// nothing else is there.
pub const RANK_USB: u8 = 2;
pub const RANK_JACK: u8 = 3;

/// Lengths a driver may give; longer is refused.
pub const ID_MAX: usize = 64;
pub const NAME_MAX: usize = 64;

/// Entries one module may hold, against one that enters without end.
const PER_OWNER: usize = 16;

/// Who entered a device and takes it out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Kernel,
    Module(u32),
}

#[derive(Clone)]
pub struct Entry {
    pub handle: u32,
    pub kind: Kind,
    pub rank: u8,
    /// Stable across boots and replugs: bus path and identity.
    pub id: String,
    pub name: String,
    pub owner: Owner,
}

struct Register {
    entries: Vec<Entry>,
    next: u32,
}

static REG: Mutex<Register> = Mutex::new(Register { entries: Vec::new(), next: 1 });

fn changed() {
    crate::notify::notify(crate::notify::TOPIC_DEVICES);
}

fn printable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control())
}

/// Enter a device; its handle, or None for a bad id or name, an id already
/// entered, or an owner past its share.
pub fn register(kind: Kind, rank: u8, id: &str, name: &str, owner: Owner) -> Option<u32> {
    if id.is_empty() || id.len() > ID_MAX || name.len() > NAME_MAX || !printable(id) || !printable(name) {
        return None;
    }
    let handle = {
        let mut r = REG.lock();
        if r.entries.iter().any(|e| e.kind == kind && e.id == id) {
            return None;
        }
        if let Owner::Module(_) = owner {
            if r.entries.iter().filter(|e| e.owner == owner).count() >= PER_OWNER {
                return None;
            }
        }
        let handle = r.next;
        r.next = r.next.wrapping_add(1).max(1);
        r.entries.push(Entry { handle, kind, rank: rank.min(RANK_JACK), id: id.into(), name: name.into(), owner });
        handle
    };
    crate::kprintln!("[npk] devices: {:?} \"{}\" ({})", kind, name, id);
    changed();
    Some(handle)
}

/// Take out `handle`; only its owner may.
pub fn remove(handle: u32, owner: Owner) -> bool {
    let gone = {
        let mut r = REG.lock();
        let Some(i) = r.entries.iter().position(|e| e.handle == handle && e.owner == owner) else { return false };
        r.entries.remove(i)
    };
    crate::kprintln!("[npk] devices: {:?} \"{}\" gone", gone.kind, gone.name);
    changed();
    true
}

/// A module ended: what it entered goes with it.
pub fn release_owner(pid: u32) {
    let n = {
        let mut r = REG.lock();
        let before = r.entries.len();
        r.entries.retain(|e| e.owner != Owner::Module(pid));
        before - r.entries.len()
    };
    if n > 0 {
        changed();
    }
}

/// The active entry of `kind`, chosen as the module doc says.
fn active_in(entries: &[Entry], kind: Kind, picked: Option<&str>) -> Option<u32> {
    let mut of_kind = entries.iter().filter(|e| e.kind == kind);
    if let Some(p) = picked {
        if let Some(e) = entries.iter().find(|e| e.kind == kind && e.id == p) {
            return Some(e.handle);
        }
    }
    let first = of_kind.next()?;
    let best = of_kind.fold(first, |b, e| if e.rank > b.rank { e } else { b });
    Some(best.handle)
}

/// The active handle of `kind`.
pub fn active(kind: Kind) -> Option<u32> {
    let picked = crate::config::get(kind.key());
    active_in(&REG.lock().entries, kind, picked.as_deref())
}

/// Whether `handle` is the active one of its kind.
pub fn is_active(handle: u32) -> Option<bool> {
    let picked: [Option<String>; 3] = [Kind::AudioOut, Kind::AudioIn, Kind::Camera].map(|k| crate::config::get(k.key()));
    let r = REG.lock();
    let e = r.entries.iter().find(|e| e.handle == handle)?;
    Some(active_in(&r.entries, e.kind, picked[e.kind as usize].as_deref()) == Some(handle))
}

/// The entry `handle`, copied out.
pub fn get(handle: u32) -> Option<Entry> {
    REG.lock().entries.iter().find(|e| e.handle == handle).cloned()
}

/// Every entry of `kind` in the order entered, and which is active.
pub fn list(kind: Kind) -> (Vec<Entry>, Option<u32>) {
    let picked = crate::config::get(kind.key());
    let r = REG.lock();
    let of_kind: Vec<Entry> = r.entries.iter().filter(|e| e.kind == kind).cloned().collect();
    (of_kind, active_in(&r.entries, kind, picked.as_deref()))
}

/// The user picks `handle` as the active one of its kind; kept by id.
pub fn select(handle: u32) -> bool {
    let Some(e) = get(handle) else { return false };
    crate::config::set(e.kind.key(), &e.id);
    crate::kprintln!("[npk] devices: {:?} now \"{}\"", e.kind, e.name);
    changed();
    true
}
