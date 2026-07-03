//! nopeek_widgets — declarative GUI SDK for WASM apps.
//!
//! Mirrors the frozen widget ABI from `kernel/src/shade/widgets/abi.rs`.
//! Apps build a `Widget` tree, call [`wire::encode`] to produce a
//! version-prefixed byte buffer, and commit it via `npk_scene_commit`.
//!
//! See `PHASE10_WIDGETS.md` for the full architecture.
//!
//! # Layering rules
//!
//! - Types in this crate are **ABI** — variant order, struct field order,
//!   and `#[repr(u8/u16)]` discriminants are frozen. Changes break every
//!   serialized tree.
//! - Postcard serializes enum variants by declaration position, so
//!   inserting a variant shifts every subsequent wire index. New variants
//!   must be **appended only**.
//! - Reserved slots (`Popover`/`Tooltip`/`Menu`, `.blur`/`.shadow`/
//!   `.effect`/`.role`) are declared now so v2 can implement them
//!   without a wire-version bump.
//!
//! # Example
//!
//! ```ignore
//! use nopeek_widgets::*;
//!
//! let tree = Widget::Column {
//!     children: alloc::vec![
//!         Widget::Text {
//!             content: "Hello nopeekOS".into(),
//!             style: TextStyle::Title,
//!             modifiers: alloc::vec::Vec::new(),
//!         },
//!     ],
//!     spacing: 8,
//!     align: Align::Start,
//!     modifiers: alloc::vec::Vec::new(),
//! };
//!
//! let bytes = wire::encode(&tree).expect("serialize");
//! // then: host::scene_commit(&bytes);
//! ```

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod abi;
pub mod app_meta;
pub mod prefab;
pub mod style;
pub mod wire;

// Launcher/dock data source. wasm32-only — it imports host fns, so it's
// compiled out of host-side test builds of the SDK.
#[cfg(target_arch = "wasm32")]
pub mod app_catalog;

// Compile-time ABI ordering guard. Mirrors kernel/src/shade/widgets/check_abi.rs.
mod check_abi;

/// Per-app capability declaration. An app embeds a 1-byte `.npk.caps`
/// custom section combining these bits; the kernel spawn path grants
/// exactly those rights (no blanket WRITE). An app with no section gets
/// a safe default (read + execute + render, never write).
///
/// ```ignore
/// #[unsafe(link_section = ".npk.caps")]
/// #[used]
/// static NPK_CAPS: [u8; 1] = [caps::READ | caps::WRITE | caps::RENDER];
/// ```
///
/// Bit layout mirrors `capability::CAP_BIT_*` in the kernel.
pub mod caps {
    /// `npk_fetch` / `npk_fs_list` / `npk_fs_stat`.
    pub const READ:   u8 = 0x01;
    /// `npk_store` / `npk_fs_delete` / `npk_set_wallpaper` / `npk_set_theme`.
    pub const WRITE:  u8 = 0x02;
    /// `npk_run_intent` / driver binds.
    pub const EXEC:   u8 = 0x04;
    /// `npk_scene_commit` / `npk_event_poll` / window + launcher fns.
    pub const RENDER: u8 = 0x08;
    /// `npk_capture_screen` — read the composited framebuffer (screenshot).
    pub const CAPTURE: u8 = 0x10;
    /// `npk_canvas_commit` — upload a raw BGRA bitmap into a `Widget::Canvas`.
    pub const CANVAS: u8 = 0x20;
    /// `npk_acpi_dsdt` / `npk_ec_read` / `npk_ec_write` — raw firmware + EC
    /// access (the AML battery driver). Highly privileged: direct hardware I/O.
    pub const HARDWARE: u8 = 0x40;
    /// `npk_wifi_send_cmd` / `npk_wifi_poll_event` — WiFi-class control channel
    /// manager side (the wifid supplicant). See WIFI_CLASS_ABI.md.
    pub const NETCTL: u8 = 0x80;

    /// Second `.npk.caps` byte — the first byte's 8 bits above are full.
    /// An app needing one of these ships a **2-byte** section:
    /// ```ignore
    /// #[unsafe(link_section = ".npk.caps")]
    /// #[used]
    /// static NPK_CAPS: [u8; 2] = [caps::RENDER | caps::CANVAS, caps::ext::NET];
    /// ```
    pub mod ext {
        /// `npk_http_request` — outbound HTTPS fetch (the native browser, beak).
        pub const NET: u8 = 0x01;
    }
}

// Re-export the core ABI types at the crate root for ergonomic use.
pub use abi::{
    Action, ActionId, Align, Axis, CanvasId, Density, EffectId, Event, Fill,
    IconId, KeyCode, Modifier, MouseButton, NodeId, Palette, Point, Rect,
    Role, Shadow, Size, Span, TextStyle, Token, Transition, Widget,
};
pub use app_meta::{AppMeta, AppMetaError, IconRef, APP_META_WIRE};
pub use style::{Elevation, Motion, Padding, Radius, Spacing};
pub use wire::{decode, encode, WireError, WIRE_VERSION};
