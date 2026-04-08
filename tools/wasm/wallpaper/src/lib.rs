//! Wallpaper WASM module for nopeekOS.
//!
//! Decodes PNG images and sets them as wallpaper via host functions.
//! Runs inside the nopeekOS WASM sandbox (wasmi).

#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

// --- Host function bindings (provided by nopeekOS kernel) ---

unsafe extern "C" {
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_set_wallpaper(ptr: i32, len: i32, width: i32, height: i32) -> i32;
    fn npk_log(ptr: i32, len: i32);
}

fn log(msg: &str) {
    unsafe { npk_log(msg.as_ptr() as i32, msg.len() as i32); }
}

fn fetch(name: &str, buf: &mut [u8]) -> Option<usize> {
    let result = unsafe {
        npk_fetch(name.as_ptr() as i32, name.len() as i32,
                  buf.as_mut_ptr() as i32, buf.len() as i32)
    };
    if result < 0 { None } else { Some(result as usize) }
}

fn set_wallpaper(pixels: &[u8], w: u32, h: u32) -> bool {
    let result = unsafe {
        npk_set_wallpaper(pixels.as_ptr() as i32, pixels.len() as i32,
                          w as i32, h as i32)
    };
    result == 0
}

// --- Simple bump allocator for WASM ---

const HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MB
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

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {
        // Bump allocator: no deallocation (module runs once and exits)
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    log("[wallpaper] panic!");
    loop {}
}

// --- Entry point ---

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // 1. Read target filename from .npk-wallpaper-target
    let mut name_buf = [0u8; 512];
    let name_len = match fetch(".npk-wallpaper-target", &mut name_buf) {
        Some(n) => n,
        None => { log("[wallpaper] no target file"); return; }
    };
    let filename = match core::str::from_utf8(&name_buf[..name_len]) {
        Ok(s) => s,
        Err(_) => { log("[wallpaper] invalid filename"); return; }
    };

    // 2. Fetch the image file
    let max_size = 6 * 1024 * 1024; // 6 MB max
    let mut img_buf = vec![0u8; max_size];
    let img_len = match fetch(filename, &mut img_buf) {
        Some(n) => n,
        None => { log("[wallpaper] failed to fetch image"); return; }
    };

    // 3. Decode PNG
    let (pixels, width, height) = match decode_png(&img_buf[..img_len]) {
        Some(v) => v,
        None => { log("[wallpaper] PNG decode failed"); return; }
    };

    // 4. Set wallpaper
    if set_wallpaper(&pixels, width, height) {
        log("[wallpaper] OK");
    } else {
        log("[wallpaper] npk_set_wallpaper failed");
    }
}

// --- PNG Decoder ---

fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    // Verify PNG signature
    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }

    let mut pos = 8;
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut bit_depth: u8 = 0;
    let mut color_type: u8 = 0;
    let mut idat_data: Vec<u8> = Vec::new();

    // Parse chunks
    while pos + 12 <= data.len() {
        let chunk_len = u32::from_be_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        let chunk_type = &data[pos+4..pos+8];
        let chunk_data_start = pos + 8;
        let chunk_data_end = chunk_data_start + chunk_len;

        if chunk_data_end > data.len() { break; }

        match chunk_type {
            b"IHDR" => {
                if chunk_len < 13 { return None; }
                let d = &data[chunk_data_start..];
                width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
                bit_depth = d[8];
                color_type = d[9];
                let compression = d[10];
                let filter = d[11];
                let interlace = d[12];

                if compression != 0 || filter != 0 || interlace != 0 {
                    log("[wallpaper] unsupported PNG format (interlaced)");
                    return None;
                }
                if bit_depth != 8 {
                    log("[wallpaper] only 8-bit PNG supported");
                    return None;
                }
                if color_type != 2 && color_type != 6 {
                    // 2 = RGB, 6 = RGBA
                    log("[wallpaper] only RGB/RGBA PNG supported");
                    return None;
                }
            }
            b"IDAT" => {
                idat_data.extend_from_slice(&data[chunk_data_start..chunk_data_end]);
            }
            b"IEND" => break,
            _ => {} // Skip unknown chunks
        }

        pos = chunk_data_end + 4; // +4 for CRC
    }

    if width == 0 || height == 0 || idat_data.is_empty() {
        return None;
    }

    // Channels per pixel
    let channels: usize = match color_type {
        2 => 3, // RGB
        6 => 4, // RGBA
        _ => return None,
    };

    let stride = width as usize * channels; // bytes per row (without filter byte)

    // Decompress IDAT (zlib = 2-byte header + deflate + 4-byte checksum)
    if idat_data.len() < 6 { return None; }
    // Skip zlib header (2 bytes), decompress deflate stream
    let deflate_data = &idat_data[2..];
    let decompressed = match miniz_oxide::inflate::decompress_to_vec_zlib(&idat_data) {
        Ok(d) => d,
        Err(_) => {
            // Try raw deflate without zlib wrapper
            match miniz_oxide::inflate::decompress_to_vec(deflate_data) {
                Ok(d) => d,
                Err(_) => { log("[wallpaper] deflate failed"); return None; }
            }
        }
    };

    let expected = height as usize * (1 + stride); // 1 filter byte per row
    if decompressed.len() < expected {
        log("[wallpaper] decompressed size mismatch");
        return None;
    }

    // Unfilter rows
    let mut unfiltered = vec![0u8; height as usize * stride];
    for y in 0..height as usize {
        let src_offset = y * (1 + stride);
        let filter_type = decompressed[src_offset];
        let row_start = src_offset + 1;
        let dst_offset = y * stride;

        for x in 0..stride {
            let raw = decompressed[row_start + x];
            let a = if x >= channels { unfiltered[dst_offset + x - channels] } else { 0 }; // left
            let b = if y > 0 { unfiltered[dst_offset - stride + x] } else { 0 }; // up
            let c = if x >= channels && y > 0 { unfiltered[dst_offset - stride + x - channels] } else { 0 }; // up-left

            let val = match filter_type {
                0 => raw,                                          // None
                1 => raw.wrapping_add(a),                         // Sub
                2 => raw.wrapping_add(b),                         // Up
                3 => raw.wrapping_add(((a as u16 + b as u16) / 2) as u8), // Average
                4 => raw.wrapping_add(paeth(a, b, c)),            // Paeth
                _ => raw,
            };
            unfiltered[dst_offset + x] = val;
        }
    }

    // Convert to BGRA (framebuffer format)
    let pixel_count = (width * height) as usize;
    let mut bgra = vec![0u8; pixel_count * 4];
    for i in 0..pixel_count {
        let src = i * channels;
        let dst = i * 4;
        bgra[dst]     = unfiltered[src + 2]; // B
        bgra[dst + 1] = unfiltered[src + 1]; // G
        bgra[dst + 2] = unfiltered[src];     // R
        bgra[dst + 3] = if channels == 4 { unfiltered[src + 3] } else { 255 }; // A
    }

    Some((bgra, width, height))
}

/// Paeth predictor (PNG filter type 4).
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).unsigned_abs();
    let pb = (p - b as i16).unsigned_abs();
    let pc = (p - c as i16).unsigned_abs();
    if pa <= pb && pa <= pc { a }
    else if pb <= pc { b }
    else { c }
}
