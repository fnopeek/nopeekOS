//! files-stub — P10.2 dummy GUI app.
//!
//! Builds a small fake file-browser widget tree using the
//! `nopeek_widgets` SDK, serializes it, and hands it to the kernel via
//! `npk_scene_commit`. No real rendering happens yet — the kernel
//! simply deserializes and pretty-prints the tree to serial.
//!
//! This is the first end-to-end use of the widget wire:
//!   SDK types → postcard encode → WASM heap → npk_scene_commit →
//!   kernel deserialize → shade::widgets::debug::print_tree.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec;

use nopeek_widgets::*;

// ── Host bindings ─────────────────────────────────────────────────────

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_log(ptr: i32, len: i32);
}

fn log(msg: &str) {
    unsafe { npk_log(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

// ── Bump allocator ────────────────────────────────────────────────────

const HEAP_SIZE: usize = 256 * 1024; // 256 KB — way more than a dummy tree needs
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;

unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + align - 1) & !(align - 1);
        if aligned + size > HEAP_SIZE {
            return core::ptr::null_mut();
        }
        unsafe { pos_ptr.write(aligned + size); }
        let heap_ptr = core::ptr::addr_of_mut!(HEAP) as *mut u8;
        unsafe { heap_ptr.add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[files-stub] panic!");
    loop {}
}

// ── Entry point ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let tree = build_tree();

    match wire::encode(&tree) {
        Ok(bytes) => {
            log("[files-stub] tree built, committing...");
            match commit(&bytes) {
                0  => log("[files-stub] commit OK"),
                -1 => log("[files-stub] commit: version mismatch / cap denied"),
                -2 => log("[files-stub] commit: deserialize failed"),
                _  => log("[files-stub] commit: unexpected error"),
            }
        }
        Err(_) => log("[files-stub] wire::encode failed"),
    }
}

// ── Sample tree ───────────────────────────────────────────────────────

/// A miniature file-browser layout: toolbar + sidebar + list + status.
fn build_tree() -> Widget {
    let entry = |icon, name: &str, size: &str| -> Widget {
        Widget::Row {
            children: vec![
                Widget::Icon {
                    id: icon,
                    size: 16,
                    modifiers: vec![],
                },
                Widget::Text {
                    content: name.to_string(),
                    style: TextStyle::Body,
                    modifiers: vec![],
                },
                Widget::Spacer { flex: 1 },
                Widget::Text {
                    content: size.to_string(),
                    style: TextStyle::Muted,
                    modifiers: vec![Modifier::Opacity(180)],
                },
            ],
            spacing: 6,
            align: Align::Center,
            modifiers: vec![],
        }
    };

    Widget::Column {
        children: vec![
            // Toolbar
            Widget::Row {
                children: vec![
                    Widget::Button {
                        label: "back".to_string(),
                        icon: IconId::ArrowLeft,
                        on_click: ActionId(1),
                        modifiers: vec![],
                    },
                    Widget::Button {
                        label: "up".to_string(),
                        icon: IconId::ArrowUp,
                        on_click: ActionId(2),
                        modifiers: vec![],
                    },
                    Widget::Text {
                        content: "/home/florian".to_string(),
                        style: TextStyle::Body,
                        modifiers: vec![Modifier::Padding(4)],
                    },
                    Widget::Spacer { flex: 1 },
                ],
                spacing: 4,
                align: Align::Center,
                modifiers: vec![Modifier::Background(Token::SurfaceElevated)],
            },

            // Body: sidebar + list
            Widget::Row {
                children: vec![
                    Widget::Column {
                        children: vec![
                            Widget::Text {
                                content: "Home".to_string(),
                                style: TextStyle::Caption,
                                modifiers: vec![],
                            },
                            Widget::Text {
                                content: "Documents".to_string(),
                                style: TextStyle::Caption,
                                modifiers: vec![],
                            },
                            Widget::Text {
                                content: "Downloads".to_string(),
                                style: TextStyle::Caption,
                                modifiers: vec![],
                            },
                        ],
                        spacing: 2,
                        align: Align::Start,
                        modifiers: vec![
                            Modifier::Background(Token::SurfaceMuted),
                            Modifier::Padding(8),
                        ],
                    },
                    Widget::Scroll {
                        child: Box::new(Widget::Column {
                            children: vec![
                                entry(IconId::Folder, "Projects", "—"),
                                entry(IconId::Folder, "Photos",   "—"),
                                entry(IconId::File,   "notes.md", "2 KB"),
                                entry(IconId::File,   "todo.txt", "184 B"),
                                Widget::Divider,
                                Widget::Checkbox {
                                    value: true,
                                    on_toggle: ActionId(10),
                                    modifiers: vec![],
                                },
                                Widget::Input {
                                    value: "".to_string(),
                                    placeholder: "Filter…".to_string(),
                                    on_submit: ActionId(11),
                                    modifiers: vec![Modifier::Padding(4)],
                                },
                            ],
                            spacing: 2,
                            align: Align::Stretch,
                            modifiers: vec![],
                        }),
                        axis: Axis::Vertical,
                        modifiers: vec![],
                    },
                ],
                spacing: 0,
                align: Align::Stretch,
                modifiers: vec![],
            },

            // Status bar
            Widget::Row {
                children: vec![
                    Widget::Text {
                        content: "4 items".to_string(),
                        style: TextStyle::Caption,
                        modifiers: vec![Modifier::Opacity(160)],
                    },
                    Widget::Spacer { flex: 1 },
                    Widget::Text {
                        content: "2.2 KB".to_string(),
                        style: TextStyle::Caption,
                        modifiers: vec![Modifier::Opacity(160)],
                    },
                ],
                spacing: 8,
                align: Align::Center,
                modifiers: vec![
                    Modifier::Background(Token::SurfaceElevated),
                    Modifier::Padding(4),
                ],
            },
        ],
        spacing: 0,
        align: Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::Surface),
            Modifier::Transition(Transition::Spring),
        ],
    }
}
