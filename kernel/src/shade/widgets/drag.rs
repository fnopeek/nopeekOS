//! Drag and drop of paths between widget windows.
//!
//! An app starts a drag with `npk_drag_start` while the left button is
//! held in its focused window; the compositor owns it from then on. On
//! release it hit-tests the window under the pointer for the deepest
//! `Modifier::DropTarget` and sends that window `Event::Drop`. Esc or a
//! release over nothing drops it.
//!
//! Only a window of the module that started the drag receives the drop.
//! Handing paths to another module would hand it access to them, which
//! needs a capability grant per path; until that exists the paths stay
//! inside the module that could already read them.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::abi;

struct Drag {
    module: String,
    paths:  Vec<String>,
}

static DRAG: Mutex<Option<Drag>> = Mutex::new(None);

/// The module that owns each widget window, noted on its scene commits.
static MODULE_OF: Mutex<BTreeMap<u32, String>> = Mutex::new(BTreeMap::new());

/// Record the module of a widget window, once. An empty name is no
/// identity and is not recorded.
pub fn note_window(window_id: u32, module: &str) {
    if module.is_empty() { return; }
    MODULE_OF.lock().entry(window_id).or_insert_with(|| String::from(module));
}

pub fn forget_window(window_id: u32) {
    MODULE_OF.lock().remove(&window_id);
}

/// Start a drag of `paths` for `module`. Replaces one still running.
pub fn start(module: &str, paths: Vec<String>) -> bool {
    if module.is_empty() { return false; }
    *DRAG.lock() = Some(Drag { module: String::from(module), paths });
    true
}

pub fn active() -> bool {
    DRAG.lock().is_some()
}

pub fn cancel() {
    DRAG.lock().take();
}

/// The left button came up at screen `(x, y)` over `window`. Ends the
/// drag; true if one was running.
pub fn release(window: Option<u32>, x: i32, y: i32, copy: bool) -> bool {
    let Some(drag) = DRAG.lock().take() else { return false };
    let Some(wid) = window else { return true };
    let same = MODULE_OF.lock().get(&wid).is_some_and(|m| *m == drag.module);
    if !same { return true; }
    if let Some(target) = super::drop_target(wid, x, y) {
        super::push_event(wid, abi::Event::Drop { target, paths: drag.paths, copy });
    }
    true
}
