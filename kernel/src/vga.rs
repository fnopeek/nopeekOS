//! VGA Text Mode
//!
//! Visual boot status indicator for QEMU/VirtualBox window.
//! Will be replaced by framebuffer canvas in Phase 8.

const VGA_BUFFER: *mut u8 = 0xB8000 as *mut u8;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

const COLOR_NORMAL: u8 = 0x07;  // light gray on black
const COLOR_BRIGHT: u8 = 0x0F;  // white on black
const COLOR_CYAN: u8 = 0x0B;
const COLOR_GREEN: u8 = 0x0A;
const COLOR_DARK: u8 = 0x08;    // dark gray on black

static mut VGA_ROW: usize = 0;

pub fn clear() {
    unsafe {
        for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
            *VGA_BUFFER.add(i * 2) = b' ';
            *VGA_BUFFER.add(i * 2 + 1) = COLOR_NORMAL;
        }
        VGA_ROW = 0;
    }
}

fn write_line(text: &[u8], color: u8) {
    unsafe {
        let row = VGA_ROW;
        if row >= VGA_HEIGHT { return; }
        for (i, &byte) in text.iter().enumerate() {
            if i >= VGA_WIDTH { break; }
            let offset = (row * VGA_WIDTH + i) * 2;
            *VGA_BUFFER.add(offset) = byte;
            *VGA_BUFFER.add(offset + 1) = color;
        }
        VGA_ROW += 1;
    }
}

fn blank_line() {
    unsafe { VGA_ROW += 1; }
}

pub fn show_boot_banner() {
    clear();
    blank_line();
    write_line(b"                                __   ____  _____", COLOR_CYAN);
    write_line(b"   ____  ____  ____  ___  ___  / /__/ __ \\/ ___/", COLOR_CYAN);
    write_line(b"  / __ \\/ __ \\/ __ \\/ _ \\/ _ \\/ //_/ / / /\\__ \\ ", COLOR_CYAN);
    write_line(b" / / / / /_/ / /_/ /  __/  __/ ,< / /_/ /___/ / ", COLOR_CYAN);
    write_line(b"/_/ /_/\\____/ .___/\\___/\\___/_/|_|\\____//____/  ", COLOR_CYAN);
    write_line(b"           /_/", COLOR_CYAN);
    blank_line();
    write_line(b"  nopeekOS - AI-native Operating System v0.1.0", COLOR_BRIGHT);
    write_line(b"  Phase 1 - Bare Metal Boot", COLOR_DARK);
    blank_line();
}

pub fn show_status(label: &[u8]) {
    unsafe {
        let row = VGA_ROW;
        if row >= VGA_HEIGHT { return; }

        let mut col = 0;
        for &byte in b"  [" {
            let offset = (row * VGA_WIDTH + col) * 2;
            *VGA_BUFFER.add(offset) = byte;
            *VGA_BUFFER.add(offset + 1) = COLOR_NORMAL;
            col += 1;
        }
        for &byte in b"OK" {
            let offset = (row * VGA_WIDTH + col) * 2;
            *VGA_BUFFER.add(offset) = byte;
            *VGA_BUFFER.add(offset + 1) = COLOR_GREEN;
            col += 1;
        }
        for &byte in b"] " {
            let offset = (row * VGA_WIDTH + col) * 2;
            *VGA_BUFFER.add(offset) = byte;
            *VGA_BUFFER.add(offset + 1) = COLOR_NORMAL;
            col += 1;
        }
        for &byte in label {
            if col >= VGA_WIDTH { break; }
            let offset = (row * VGA_WIDTH + col) * 2;
            *VGA_BUFFER.add(offset) = byte;
            *VGA_BUFFER.add(offset + 1) = COLOR_NORMAL;
            col += 1;
        }
        VGA_ROW += 1;
    }
}

pub fn show_ready() {
    blank_line();
    write_line(b"  Serial console active on COM1", COLOR_DARK);
    write_line(b"  Express your intent.", COLOR_BRIGHT);
}
