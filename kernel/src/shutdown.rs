//! Orderly power-off and restart. Every app window is asked to close the
//! way the user's close gesture asks it, so an app with unsaved work can
//! prompt and the microVM's guest can end its app, sync and power off, and
//! its home image is saved. Only when no app window and no guest is left
//! are npkFS synced and the machine switched off or reset. An app that
//! stays open (the user cancelled its unsaved-work dialog) cancels the
//! shutdown.
//!
//! Driven from Core 0's desktop loop (`tick`): the guest's teardown runs
//! there too, so waiting in place would wait forever.

use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    PowerOff,
    Restart,
}

struct Pending {
    kind: Kind,
    /// When the windows were asked (`interrupts::ticks`).
    since: u64,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// How long a guest may take after the last window closed before the
/// machine goes down anyway (ticks, 100 Hz). The microVM's own grace has
/// stopped it well before this.
const GUEST_GRACE_TICKS: u64 = 60 * 100;

/// Begin a power-off or restart. Without the desktop there is nothing to
/// ask, and it happens at once.
pub fn request(kind: Kind) {
    if !crate::shade::is_active() {
        finish(kind);
    }
    {
        let mut p = PENDING.lock();
        if p.is_some() {
            return;
        }
        *p = Some(Pending { kind, since: crate::interrupts::ticks() });
    }
    // A cancel left over from an earlier close must not stop this one.
    let _ = crate::shade::widgets::take_close_cancelled();
    let asked = crate::shade::with_compositor(|c| c.request_close_all_apps()).unwrap_or(0);
    crate::kprintln!("[npk] {}: asking {} app window(s) to close",
        if kind == Kind::Restart { "restart" } else { "power off" }, asked);
    crate::shade::request_render();
}

/// One step, from Core 0's desktop loop.
pub fn tick() {
    let Some((kind, since)) = PENDING.lock().as_ref().map(|p| (p.kind, p.since)) else { return };
    if let Some(id) = crate::shade::widgets::take_close_cancelled() {
        let title = crate::shade::with_compositor(|c| c.window_title(crate::shade::window::WindowId(id)))
            .flatten().unwrap_or_default();
        crate::kprintln!("[npk] {} cancelled: '{}' stays open",
            if kind == Kind::Restart { "restart" } else { "power off" }, title);
        *PENDING.lock() = None;
        return;
    }
    let apps = crate::shade::with_compositor(|c| c.app_windows_left()).unwrap_or(0);
    if apps > 0 {
        return;
    }
    let guest = crate::microvm::vm_busy();
    if guest && crate::interrupts::ticks().saturating_sub(since) < GUEST_GRACE_TICKS {
        return;
    }
    if guest {
        crate::kprintln!("[npk] the guest did not end in time; going down anyway");
    }
    finish(kind);
}

fn finish(kind: Kind) -> ! {
    match kind {
        Kind::PowerOff => crate::intent::system::intent_halt(),
        Kind::Restart => crate::intent::system::intent_reboot(),
    }
}
