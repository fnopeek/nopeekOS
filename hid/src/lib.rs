//! hid_core — HID report descriptors and input reports.
//!
//! Ported from Linux `drivers/hid/hid-core.c`: the item parser
//! (`hid_parser_main`/`_global`/`_local`, `hid_add_field`, collections,
//! `hid_concatenate_last_usage_page`) and the input path
//! (`hid_report_raw_event`, `hid_process_report`, `hid_input_fetch_field`,
//! the variable and array field handlers). Fields keep Linux's shape: one
//! field per main item with a usage table, so an array field (a keyboard's
//! key slots, a consumer control) reports the usage its value indexes.
//!
//! [`input`] is the part of `hid-input.c` that turns those usages into what
//! the kernel consumes: keyboard usages, mouse buttons and motion, volume.
//!
//! Not ported: report fixups and quirks (no device here needs one),
//! resolution multipliers, delimiter branches past the first, the usage
//! priority ordering (`hid_process_ordering`; it only reorders digitizer
//! usages, every keyboard and mouse usage has priority 0), and the output
//! path beyond building a report from field values.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod input;

use alloc::vec;
use alloc::vec::Vec;

pub const HID_MAX_USAGES: usize = 12288;
pub const HID_MAX_IDS: u32 = 256;
pub const HID_MAX_FIELDS: usize = 256;
pub const HID_MAX_BUFFER_SIZE: usize = 16384;
pub const HID_MIN_BUFFER_SIZE: usize = 64;
const HID_GLOBAL_STACK_SIZE: usize = 4;

pub const MAIN_ITEM_CONSTANT: u32 = 0x001;
pub const MAIN_ITEM_VARIABLE: u32 = 0x002;
pub const MAIN_ITEM_RELATIVE: u32 = 0x004;

pub const COLLECTION_PHYSICAL: u32 = 0;
pub const COLLECTION_APPLICATION: u32 = 1;
pub const COLLECTION_LOGICAL: u32 = 2;

pub const UP_GENDESK: u32 = 0x0001_0000;
pub const UP_KEYBOARD: u32 = 0x0007_0000;
pub const UP_LED: u32 = 0x0008_0000;
pub const UP_BUTTON: u32 = 0x0009_0000;
pub const UP_CONSUMER: u32 = 0x000c_0000;
pub const USAGE_PAGE: u32 = 0xffff_0000;
pub const USAGE: u32 = 0x0000_ffff;

pub const GD_POINTER: u32 = 0x0001_0001;
pub const GD_MOUSE: u32 = 0x0001_0002;
pub const GD_KEYBOARD: u32 = 0x0001_0006;

// Item tags (`include/linux/hid.h`).
const ITEM_TYPE_MAIN: u8 = 0;
const ITEM_TYPE_GLOBAL: u8 = 1;
const ITEM_TYPE_LOCAL: u8 = 2;
const ITEM_TAG_LONG: u8 = 15;

const MAIN_TAG_INPUT: u8 = 8;
const MAIN_TAG_OUTPUT: u8 = 9;
const MAIN_TAG_FEATURE: u8 = 11;
const MAIN_TAG_BEGIN_COLLECTION: u8 = 10;
const MAIN_TAG_END_COLLECTION: u8 = 12;

const GLOBAL_TAG_USAGE_PAGE: u8 = 0;
const GLOBAL_TAG_LOGICAL_MINIMUM: u8 = 1;
const GLOBAL_TAG_LOGICAL_MAXIMUM: u8 = 2;
const GLOBAL_TAG_PHYSICAL_MINIMUM: u8 = 3;
const GLOBAL_TAG_PHYSICAL_MAXIMUM: u8 = 4;
const GLOBAL_TAG_UNIT_EXPONENT: u8 = 5;
const GLOBAL_TAG_UNIT: u8 = 6;
const GLOBAL_TAG_REPORT_SIZE: u8 = 7;
const GLOBAL_TAG_REPORT_ID: u8 = 8;
const GLOBAL_TAG_REPORT_COUNT: u8 = 9;
const GLOBAL_TAG_PUSH: u8 = 10;
const GLOBAL_TAG_POP: u8 = 11;

const LOCAL_TAG_USAGE: u8 = 0;
const LOCAL_TAG_USAGE_MINIMUM: u8 = 1;
const LOCAL_TAG_USAGE_MAXIMUM: u8 = 2;
const LOCAL_TAG_DELIMITER: u8 = 10;

/// `enum hid_report_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportType {
    Input = 0,
    Output = 1,
    Feature = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// An item the parser refuses, at this byte offset.
    Item(usize),
    /// The descriptor ends inside an item.
    Truncated(usize),
    UnbalancedCollection,
    UnbalancedDelimiter,
    /// A report shorter than its descriptor says, or with no ID byte.
    ShortReport,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    /// Usage page in the high 16 bits, usage ID in the low.
    pub hid: u32,
    pub collection_index: u32,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub usage: Vec<Usage>,
    pub flags: u32,
    /// Bit offset in the report, after the ID byte.
    pub report_offset: u32,
    pub report_size: u32,
    pub report_count: u32,
    pub logical_minimum: i32,
    pub logical_maximum: i32,
    pub physical_minimum: i32,
    pub physical_maximum: i32,
    pub unit_exponent: i32,
    pub unit: u32,
    pub application: u32,
    pub physical: u32,
    pub logical: u32,
    value: Vec<i32>,
    new_value: Vec<i32>,
    ignored: bool,
}

impl Field {
    pub fn is_variable(&self) -> bool { self.flags & MAIN_ITEM_VARIABLE != 0 }
    pub fn is_relative(&self) -> bool { self.flags & MAIN_ITEM_RELATIVE != 0 }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub id: u32,
    pub kind: ReportType,
    /// Size in bits, without the ID byte.
    pub size: u32,
    pub application: u32,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Default)]
pub struct ReportEnum {
    /// Reports carry an ID byte.
    pub numbered: bool,
    /// In the order they were first named, as `report_list`.
    pub reports: Vec<Report>,
}

impl ReportEnum {
    pub fn get(&self, id: u32) -> Option<&Report> {
        self.reports.iter().find(|r| r.id == id)
    }
    fn index(&self, id: u32) -> Option<usize> {
        self.reports.iter().position(|r| r.id == id)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Collection {
    pub parent_idx: i32,
    pub kind: u32,
    pub usage: u32,
    pub level: u32,
}

/// A parsed device: `struct hid_device` as far as reports go.
#[derive(Clone, Debug, Default)]
pub struct Device {
    pub report_enum: [ReportEnum; 3],
    pub collection: Vec<Collection>,
}

/// One usage changing value — what `hid_process_event` hands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub usage: u32,
    pub value: i32,
    pub flags: u32,
    pub application: u32,
    pub physical: u32,
}

#[derive(Clone, Copy, Default)]
struct Global {
    usage_page: u32,
    logical_minimum: i32,
    logical_maximum: i32,
    physical_minimum: i32,
    physical_maximum: i32,
    unit_exponent: i32,
    unit: u32,
    report_id: u32,
    report_size: u32,
    report_count: u32,
}

#[derive(Default)]
struct Local {
    usage: Vec<u32>,
    usage_size: Vec<u8>,
    collection_index: Vec<u32>,
    usage_minimum: u32,
    delimiter_depth: u32,
    delimiter_branch: u32,
}

impl Local {
    fn usage_index(&self) -> usize { self.usage.len() }
}

struct Parser {
    device: Device,
    global: Global,
    global_stack: Vec<Global>,
    local: Local,
    collection_stack: Vec<u32>,
}

/// `struct hid_item`, short items only.
struct Item {
    format_long: bool,
    size: u8,
    kind: u8,
    tag: u8,
    data: u32,
}

impl Item {
    fn udata(&self) -> u32 {
        if self.format_long { return 0; }
        match self.size {
            1 => self.data & 0xff,
            2 => self.data & 0xffff,
            4 => self.data,
            _ => 0,
        }
    }
    fn sdata(&self) -> i32 {
        if self.format_long { return 0; }
        match self.size {
            1 => self.data as u8 as i8 as i32,
            2 => self.data as u16 as i16 as i32,
            4 => self.data as i32,
            _ => 0,
        }
    }
}

/// `fetch_item`: the item at `start`, and where the next one begins.
fn fetch_item(desc: &[u8], start: usize) -> Option<(Item, usize)> {
    let b = *desc.get(start)?;
    let mut p = start + 1;
    let kind = (b >> 2) & 3;
    let tag = (b >> 4) & 15;
    if tag == ITEM_TAG_LONG {
        if desc.len() - p < 2 { return None; }
        let size = desc[p];
        let tag = desc[p + 1];
        p += 2;
        if desc.len() - p < size as usize { return None; }
        return Some((Item { format_long: true, size, kind, tag, data: 0 }, p + size as usize));
    }
    // 0, 1, 2, 3 -> 0, 1, 2, 4
    let size = (1u8 << (b & 3)) >> 1;
    if desc.len() - p < size as usize { return None; }
    let mut data = 0u32;
    for k in 0..size as usize {
        data |= (desc[p + k] as u32) << (8 * k);
    }
    p += size as usize;
    Some((Item { format_long: false, size, kind, tag, data }, p))
}

/// `snto32`: sign-extend an `n`-bit value.
pub fn snto32(value: u32, n: u32) -> i32 {
    if value == 0 || n == 0 { return 0; }
    let n = n.min(32);
    let shift = 32 - n;
    ((value << shift) as i32) >> shift
}

/// `s32ton`: a signed value into `n` bits, saturated.
pub fn s32ton(value: i32, n: u32) -> u32 {
    if value == 0 || n == 0 { return 0; }
    let n = n.min(32);
    let a = value >> (n - 1);
    if a != 0 && a != -1 {
        return if value < 0 { 1 << (n - 1) } else { (1 << (n - 1)) - 1 };
    }
    if n == 32 { value as u32 } else { (value as u32) & ((1u32 << n) - 1) }
}

/// `__extract`: `n` bits at bit `offset`, little endian. Bits past the end
/// of `report` read as zero.
pub fn field_extract(report: &[u8], offset: u32, n: u32) -> u32 {
    let n = n.min(32);
    let mut value: u64 = 0;
    let mut got = 0u32;
    let mut idx = (offset / 8) as usize;
    let mut shift = offset % 8;
    while got < n {
        let byte = *report.get(idx).unwrap_or(&0) as u64;
        value |= (byte >> shift) << got;
        got += 8 - shift;
        shift = 0;
        idx += 1;
    }
    let mask: u64 = if n < 32 { (1u64 << n) - 1 } else { 0xffff_ffff };
    (value & mask) as u32
}

/// `__implement`: write the low `n` bits of `value` at bit `offset`.
pub fn field_implement(report: &mut [u8], offset: u32, n: u32, value: u32) {
    for k in 0..n.min(32) {
        let bit = offset + k;
        let Some(byte) = report.get_mut((bit / 8) as usize) else { return };
        let mask = 1u8 << (bit % 8);
        if (value >> k) & 1 != 0 { *byte |= mask } else { *byte &= !mask }
    }
}

impl Parser {
    /// `hid_lookup_collection`.
    fn lookup_collection(&self, kind: u32) -> u32 {
        for &index in self.collection_stack.iter().rev() {
            let c = &self.device.collection[index as usize];
            if c.kind == kind { return c.usage; }
        }
        0
    }

    /// `complete_usage`.
    fn complete_usage(&mut self, index: usize) {
        self.local.usage[index] &= 0xffff;
        self.local.usage[index] |= (self.global.usage_page & 0xffff) << 16;
    }

    /// `hid_add_usage`.
    fn add_usage(&mut self, usage: u32, size: u8) -> Result<(), ()> {
        if self.local.usage_index() >= HID_MAX_USAGES { return Err(()); }
        self.local.usage.push(usage);
        let index = self.local.usage_index() - 1;
        if size <= 2 { self.complete_usage(index); }
        self.local.usage_size.push(size);
        self.local.collection_index.push(self.collection_stack.last().copied().unwrap_or(0));
        Ok(())
    }

    /// `open_collection`.
    fn open_collection(&mut self, kind: u32) {
        let usage = self.local.usage.first().copied().unwrap_or(0);
        let index = self.device.collection.len() as u32;
        let level = self.collection_stack.len() as u32;
        let parent_idx = if level == 0 { -1 } else { self.collection_stack[level as usize - 1] as i32 };
        self.collection_stack.push(index);
        self.device.collection.push(Collection { parent_idx, kind, usage, level });
    }

    /// `close_collection`.
    fn close_collection(&mut self) -> Result<(), ()> {
        self.collection_stack.pop().map(|_| ()).ok_or(())
    }

    /// `hid_add_field`.
    fn add_field(&mut self, kind: ReportType, flags: u32) -> Result<(), ()> {
        let application = self.lookup_collection(COLLECTION_APPLICATION);
        let g = self.global;
        if g.report_id >= HID_MAX_IDS { return Err(()); }

        let reports = &mut self.device.report_enum[kind as usize];
        let ri = match reports.index(g.report_id) {
            Some(i) => i,
            None => {
                if g.report_id != 0 { reports.numbered = true; }
                reports.reports.push(Report {
                    id: g.report_id, kind, size: 0, application, fields: Vec::new(),
                });
                reports.reports.len() - 1
            }
        };

        // Handle both signed and unsigned cases properly.
        if (g.logical_minimum < 0 && g.logical_maximum < g.logical_minimum)
            || (g.logical_minimum >= 0 && (g.logical_maximum as u32) < (g.logical_minimum as u32))
        {
            return Err(());
        }

        let report = &mut reports.reports[ri];
        let offset = report.size;
        report.size += g.report_size * g.report_count;

        // Total size check: allow for a possible report ID byte.
        if report.size as usize > (HID_MAX_BUFFER_SIZE - 1) << 3 { return Err(()); }

        // Padding fields are not registered.
        if self.local.usage_index() == 0 { return Ok(()); }

        if report.fields.len() == HID_MAX_FIELDS { return Ok(()); }

        let usages = self.local.usage_index().max(g.report_count as usize);
        let physical = self.lookup_collection(COLLECTION_PHYSICAL);
        let logical = self.lookup_collection(COLLECTION_LOGICAL);
        let mut usage = Vec::with_capacity(usages);
        for i in 0..usages {
            // Duplicate the last usage if there are more values than usages.
            let j = i.min(self.local.usage_index() - 1);
            usage.push(Usage {
                hid: self.local.usage[j],
                collection_index: self.local.collection_index[j],
            });
        }
        let report = &mut self.device.report_enum[kind as usize].reports[ri];
        report.fields.push(Field {
            usage,
            flags,
            report_offset: offset,
            report_size: g.report_size,
            report_count: g.report_count,
            logical_minimum: g.logical_minimum,
            logical_maximum: g.logical_maximum,
            physical_minimum: g.physical_minimum,
            physical_maximum: g.physical_maximum,
            unit_exponent: g.unit_exponent,
            unit: g.unit,
            application,
            physical,
            logical,
            value: vec![0; usages],
            new_value: vec![0; usages],
            ignored: false,
        });
        Ok(())
    }

    /// `hid_parser_global`.
    fn global(&mut self, item: &Item) -> Result<(), ()> {
        match item.tag {
            GLOBAL_TAG_PUSH => {
                if self.global_stack.len() == HID_GLOBAL_STACK_SIZE { return Err(()); }
                self.global_stack.push(self.global);
            }
            GLOBAL_TAG_POP => {
                self.global = self.global_stack.pop().ok_or(())?;
            }
            GLOBAL_TAG_USAGE_PAGE => self.global.usage_page = item.udata(),
            GLOBAL_TAG_LOGICAL_MINIMUM => self.global.logical_minimum = item.sdata(),
            GLOBAL_TAG_LOGICAL_MAXIMUM => {
                self.global.logical_maximum = if self.global.logical_minimum < 0 {
                    item.sdata()
                } else {
                    item.udata() as i32
                };
            }
            GLOBAL_TAG_PHYSICAL_MINIMUM => self.global.physical_minimum = item.sdata(),
            GLOBAL_TAG_PHYSICAL_MAXIMUM => {
                self.global.physical_maximum = if self.global.physical_minimum < 0 {
                    item.sdata()
                } else {
                    item.udata() as i32
                };
            }
            GLOBAL_TAG_UNIT_EXPONENT => {
                // Many devices give the exponent as a two's complement nibble.
                let raw = item.sdata();
                self.global.unit_exponent = if raw as u32 & 0xffff_fff0 == 0 {
                    snto32(raw as u32, 4)
                } else {
                    raw
                };
            }
            GLOBAL_TAG_UNIT => self.global.unit = item.udata(),
            GLOBAL_TAG_REPORT_SIZE => {
                self.global.report_size = item.udata();
                if self.global.report_size > 256 { return Err(()); }
            }
            GLOBAL_TAG_REPORT_COUNT => {
                self.global.report_count = item.udata();
                if self.global.report_count as usize > HID_MAX_USAGES { return Err(()); }
            }
            GLOBAL_TAG_REPORT_ID => {
                self.global.report_id = item.udata();
                if self.global.report_id == 0 || self.global.report_id >= HID_MAX_IDS {
                    return Err(());
                }
            }
            _ => return Err(()),
        }
        Ok(())
    }

    /// `hid_parser_local`.
    fn local(&mut self, item: &Item) -> Result<(), ()> {
        let data = item.udata();
        match item.tag {
            LOCAL_TAG_DELIMITER => {
                if data != 0 {
                    // Items before the first delimiter are global to every
                    // usage set; only the first set is processed.
                    if self.local.delimiter_depth != 0 { return Err(()); }
                    self.local.delimiter_depth += 1;
                    self.local.delimiter_branch += 1;
                } else {
                    if self.local.delimiter_depth < 1 { return Err(()); }
                    self.local.delimiter_depth -= 1;
                }
            }
            LOCAL_TAG_USAGE => {
                if self.local.delimiter_branch > 1 { return Ok(()); }
                self.add_usage(data, item.size)?;
            }
            LOCAL_TAG_USAGE_MINIMUM => {
                if self.local.delimiter_branch > 1 { return Ok(()); }
                self.local.usage_minimum = data;
            }
            LOCAL_TAG_USAGE_MAXIMUM => {
                if self.local.delimiter_branch > 1 { return Ok(()); }
                let mut data = data;
                let count = data.wrapping_sub(self.local.usage_minimum) as usize;
                if count + self.local.usage_index() >= HID_MAX_USAGES {
                    let room = (HID_MAX_USAGES - self.local.usage_index()) as u32;
                    data = (room + self.local.usage_minimum).wrapping_sub(1);
                    if data as i32 <= 0 { return Err(()); }
                }
                let mut n = self.local.usage_minimum;
                while n <= data {
                    self.add_usage(n, item.size)?;
                    n += 1;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `hid_concatenate_last_usage_page`.
    fn concatenate_last_usage_page(&mut self) {
        let usage_page = self.global.usage_page;
        for i in (0..self.local.usage_index()).rev() {
            if self.local.usage_size[i] > 2 { continue; }
            if self.local.usage[i] >> 16 == usage_page { break; }
            self.complete_usage(i);
        }
    }

    /// `hid_parser_main`.
    fn main(&mut self, item: &Item) -> Result<(), ()> {
        self.concatenate_last_usage_page();
        let data = item.udata();
        let ret = match item.tag {
            MAIN_TAG_BEGIN_COLLECTION => { self.open_collection(data & 0xff); Ok(()) }
            MAIN_TAG_END_COLLECTION => self.close_collection(),
            MAIN_TAG_INPUT => self.add_field(ReportType::Input, data),
            MAIN_TAG_OUTPUT => self.add_field(ReportType::Output, data),
            MAIN_TAG_FEATURE => self.add_field(ReportType::Feature, data),
            _ => Ok(()),
        };
        self.local = Local::default();
        ret
    }
}

/// `hid_open_report`: parse a report descriptor.
pub fn parse(desc: &[u8]) -> Result<Device, Error> {
    let mut p = Parser {
        device: Device::default(),
        global: Global::default(),
        global_stack: Vec::new(),
        local: Local::default(),
        collection_stack: Vec::new(),
    };
    let mut start = 0usize;
    while start < desc.len() {
        let Some((item, next)) = fetch_item(desc, start) else {
            return Err(Error::Truncated(start));
        };
        if item.format_long { return Err(Error::Item(start)); }
        let ok = match item.kind {
            ITEM_TYPE_MAIN => p.main(&item),
            ITEM_TYPE_GLOBAL => p.global(&item),
            ITEM_TYPE_LOCAL => p.local(&item),
            _ => Ok(()),
        };
        if ok.is_err() { return Err(Error::Item(start)); }
        start = next;
    }
    if !p.collection_stack.is_empty() { return Err(Error::UnbalancedCollection); }
    if p.local.delimiter_depth != 0 { return Err(Error::UnbalancedDelimiter); }
    Ok(p.device)
}

/// `hid_array_value_is_valid`.
fn array_value_is_valid(field: &Field, value: i32) -> bool {
    let min = field.logical_minimum;
    value >= min && value <= field.logical_maximum
        && ((value as i64 - min as i64) as u64) < field.usage.len() as u64
}

/// `hid_input_fetch_field`.
fn input_fetch_field(field: &mut Field, data: &[u8]) {
    let count = field.report_count as usize;
    let min = field.logical_minimum;
    field.ignored = false;
    for v in field.new_value.iter_mut() { *v = 0; }
    for n in 0..count {
        let raw = field_extract(data, field.report_offset + n as u32 * field.report_size,
            field.report_size);
        let v = if min < 0 { snto32(raw, field.report_size) } else { raw as i32 };
        field.new_value[n] = v;
        // Ignore the report on ErrorRollOver.
        if !field.is_variable() && array_value_is_valid(field, v)
            && field.usage[(v - min) as usize].hid == UP_KEYBOARD + 1
        {
            field.ignored = true;
            return;
        }
    }
}

fn event(field: &Field, usage: u32, value: i32) -> Event {
    Event { usage, value, flags: field.flags, application: field.application, physical: field.physical }
}

/// `hid_input_var_field`.
fn input_var_field(field: &mut Field, out: &mut impl FnMut(Event)) {
    let count = field.report_count as usize;
    for n in 0..count {
        out(event(field, field.usage[n].hid, field.new_value[n]));
    }
    field.value[..count].copy_from_slice(&field.new_value[..count]);
}

/// `hid_input_array_field`.
fn input_array_field(field: &mut Field, out: &mut impl FnMut(Event)) {
    let count = field.report_count as usize;
    let min = field.logical_minimum;
    if field.ignored { return; }
    for n in 0..count {
        let old = field.value[n];
        if array_value_is_valid(field, old) && !field.new_value[..count].contains(&old) {
            out(event(field, field.usage[(old - min) as usize].hid, 0));
        }
        let new = field.new_value[n];
        if array_value_is_valid(field, new) && !field.value[..count].contains(&new) {
            out(event(field, field.usage[(new - min) as usize].hid, 1));
        }
    }
    field.value[..count].copy_from_slice(&field.new_value[..count]);
}

impl Device {
    /// `hid_find_max_report`: the longest report of this type in bytes,
    /// with its ID byte. The USB layer sizes its input buffer with it.
    pub fn max_report_bytes(&self, kind: ReportType) -> usize {
        let e = &self.report_enum[kind as usize];
        e.reports.iter()
            .map(|r| ((r.size as usize).saturating_sub(1) >> 3) + 1 + e.numbered as usize)
            .max()
            .unwrap_or(0)
    }

    /// `hid_report_raw_event` + `hid_process_report` for an input report:
    /// the usages whose value is reported, in field order, through `out`.
    /// Allocation-free.
    pub fn input_report(&mut self, data: &[u8], out: &mut impl FnMut(Event)) -> Result<(), Error> {
        let e = &mut self.report_enum[ReportType::Input as usize];
        let numbered = e.numbered;
        if numbered && data.is_empty() { return Err(Error::ShortReport); }
        let id = if numbered { data[0] as u32 } else { 0 };
        let Some(ri) = e.index(id) else { return Ok(()) };
        let cdata = if numbered { &data[1..] } else { data };

        // A report shorter than `hid_compute_report_size` is padded with
        // zeros; `field_extract` reads zeros past the end, which is the same
        // without a copy. Nothing on this path allocates: it runs in the
        // USB interrupt, and the kernel heap lock does not mask interrupts.
        let report = &mut e.reports[ri];
        for field in report.fields.iter_mut() {
            input_fetch_field(field, cdata);
        }
        for field in report.fields.iter_mut() {
            if field.is_variable() {
                input_var_field(field, out);
            } else {
                input_array_field(field, out);
            }
        }
        Ok(())
    }

    /// The fields of an output report, for `hid_output_report`.
    pub fn output_report(&self, id: u32) -> Option<&Report> {
        self.report_enum[ReportType::Output as usize].get(id)
    }

    /// `hid_output_report` with every field at zero: the bytes of output
    /// report `id`, with its ID byte if the device numbers them.
    /// `usbhid_set_leds` sends this to clear the LEDs a BIOS left on.
    pub fn output_report_zeroed(&self, id: u32) -> Option<Vec<u8>> {
        let e = &self.report_enum[ReportType::Output as usize];
        let r = e.get(id)?;
        let len = ((r.size as usize) + 7) / 8;
        let mut out = vec![0u8; len + e.numbered as usize];
        if e.numbered { out[0] = id as u8; }
        Some(out)
    }

    /// `hid_find_field_early`: the output report holding usage `usage`.
    pub fn find_output_usage(&self, usage: u32) -> Option<u32> {
        self.report_enum[ReportType::Output as usize].reports.iter()
            .find(|r| r.fields.iter().any(|f| f.usage.iter().any(|u| u.hid == usage)))
            .map(|r| r.id)
    }

    /// The top-level application collections — what the device is.
    pub fn applications(&self) -> impl Iterator<Item = u32> + '_ {
        self.collection.iter()
            .filter(|c| c.kind == COLLECTION_APPLICATION && c.level == 0)
            .map(|c| c.usage)
    }
}

#[cfg(test)]
mod tests;
