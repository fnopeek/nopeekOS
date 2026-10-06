//! Module-global state without `static mut`.
//!
//! A wasm instance runs on one thread, so a `Sync` cell is sound as long as
//! no two borrows overlap. `with` enforces that at run time: a nested call
//! (only possible by re-entering from inside `f`) panics instead of
//! creating a second `&mut`.

use core::cell::{Cell, UnsafeCell};

pub struct Single<T> {
    busy: Cell<bool>,
    value: UnsafeCell<T>,
}

// SAFETY: a wasm module has one thread; `with` rules out overlapping
// borrows on it.
unsafe impl<T> Sync for Single<T> {}

impl<T> Single<T> {
    pub const fn new(value: T) -> Self {
        Single { busy: Cell::new(false), value: UnsafeCell::new(value) }
    }

    /// Run `f` with exclusive access to the value.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(!self.busy.replace(true), "Single::with re-entered");
        // SAFETY: `busy` guarantees this is the only live reference.
        let r = f(unsafe { &mut *self.value.get() });
        self.busy.set(false);
        r
    }
}

impl<T: Copy> Single<T> {
    pub fn get(&self) -> T {
        self.with(|v| *v)
    }

    pub fn set(&self, v: T) {
        self.with(|x| *x = v)
    }
}
