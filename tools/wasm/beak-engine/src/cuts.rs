//! Safety limits that cut something off a page, counted so the shell can say
//! so. The engine has no log of its own; a limit that bites without a line in
//! the log looks like a layout bug.
//!
//! Counters, not messages: a cut inside layout can repeat on every pass, and
//! the shell reports each kind once per drain with how often it happened.

use core::sync::atomic::{AtomicU32, Ordering};

/// What was cut, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// A grid template or an `auto-fill` expansion past `style::MAX_GRID_TRACKS`.
    GridTracks,
    /// A table wider than `layout::MAX_TABLE_COLS` columns.
    TableColumns,
    /// An `<img>` whose decoded pixels exceed what is left of the page budget.
    ImageBudget,
    /// A CSS image layer whose decoded pixels exceed the CSS image budget.
    CssImageBudget,
    /// An inline `<svg>` whose raster exceeds what is left of the page budget.
    SvgBudget,
    /// An SVG rasterised smaller than its box to stay inside `svg::MAX_RASTER_BYTES`.
    SvgScaled,
    /// A picture larger than the whole cross-navigation cache, not cached.
    CacheTooSmall,
    /// A `localStorage` write refused for exceeding the per-origin quota.
    StorageQuota,
}

const KINDS: [Cut; 8] = [
    Cut::GridTracks, Cut::TableColumns, Cut::ImageBudget, Cut::CssImageBudget,
    Cut::SvgBudget, Cut::SvgScaled, Cut::CacheTooSmall, Cut::StorageQuota,
];

static COUNTS: [AtomicU32; KINDS.len()] = [const { AtomicU32::new(0) }; KINDS.len()];

impl Cut {
    fn index(self) -> usize {
        KINDS.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// The log text for this kind.
    pub fn reason(self) -> &'static str {
        match self {
            Cut::GridTracks => "grid tracks cut at the safety limit",
            Cut::TableColumns => "table columns cut at the safety limit",
            Cut::ImageBudget => "image dropped: page image budget used up",
            Cut::CssImageBudget => "CSS image dropped: CSS image budget used up",
            Cut::SvgBudget => "inline SVG dropped: page image budget used up",
            Cut::SvgScaled => "SVG rasterised below its box size (raster byte limit)",
            Cut::CacheTooSmall => "image larger than the whole image cache, not cached",
            Cut::StorageQuota => "localStorage write refused: origin quota exceeded",
        }
    }
}

/// Count one cut.
pub fn note(c: Cut) {
    COUNTS[c.index()].fetch_add(1, Ordering::Relaxed);
}

/// The cuts since the last call, with how often each happened; resets them.
pub fn take() -> alloc::vec::Vec<(Cut, u32)> {
    let mut out = alloc::vec::Vec::new();
    for k in KINDS {
        let n = COUNTS[k.index()].swap(0, Ordering::Relaxed);
        if n > 0 {
            out.push((k, n));
        }
    }
    out
}
