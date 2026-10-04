# `kernel/src/drivers/battery.rs` @ 5e0102684

## L1-8 · `use crate::smbus;`

```
//! Smart Battery System (SBS) client over the i801 SMBus.
//!
//! Standardised registers from Linux `drivers/power/supply/sbs-battery.c`:
//! a smart battery answers at SMBus address 0x0B, exposing relative state
//! of charge (0x0D, 0..100 %) and a status word (0x16) whose bits classify
//! charging vs discharging. No ACPI/AML needed — but it only works if the
//! pack sits directly on the SMBus (some laptops hide it behind the EC, in
//! which case [`read`] simply returns None and the bar segment stays empty).
```

## L13-17 · `static REPORT: AtomicI32 = AtomicI32::new(-1);`

```
// Battery state reported by the AML driver (aml.wasm), encoded as the bar
// expects: (status << 8) | percent, or -1 for "no battery / no report yet".
// The driver runs the firmware's _BST/_BIF (vendor-independent) and pushes
// here; `npk_battery()` returns this. Replaces the old per-device EC offset
// hardcode (which only worked on one HP model).
```

## L20 · `pub fn report(packed: i32) {`

```
/// Called by the AML driver via `npk_battery_report`.
```

## L27-29 · `#[derive(Clone, Copy)]`

```
/// Raw `_BST`/`_BIF` figures from the AML driver, for `battery`: what the
/// whole machine draws. `rate`/`voltage` are 0xFFFFFFFF when unknown;
/// `unit` 0 = mW/mWh, 1 = mA/mAh.
```

## L41 · `pub fn report_detail(d: Detail) {`

```
/// Called by the AML driver via `npk_battery_detail`.
```

## L50-51 · `pub fn cached() -> i32 {`

```
/// The latest driver report (or -1). `npk_battery()` returns this, falling
/// back to the standardised SBS-over-SMBus path for desktops/SBS laptops.
```

## L60 · `const BATTERY_DISCHARGING: u16 = 0x40;`

```
// BatteryStatus bits (sbs-battery.c).
```

## L69-70 · `PluggedIdle = 3,`

```
/// On AC but not actively charging (e.g. HP Adaptive Battery Care holds
/// the charge at ~85 %). Shown with a plug icon, not the charging bolt.
```

## L80-84 · `pub fn read() -> Option<BatteryState> {`

```
/// Read the current battery state via the standardised SBS-over-SMBus path
/// (works when the pack sits directly on the bus). Laptops that hide the pack
/// behind the EC report through the AML driver instead — see [`cached`]. The
/// former per-device EC offset hardcode was removed in favour of aml.wasm,
/// which runs the firmware's own `_BST`/`_BIF` and is vendor-independent.
```

## L89-90 · `fn read_sbs() -> Option<BatteryState> {`

```
/// Smart Battery System over the i801 SMBus (works when the pack is wired
/// directly to the bus). None when it NAKs.
```

## L95 · `let status = match smbus::read_word(SBS_ADDR, REG_BATTERY_STATUS) {`

```
// Status read is best-effort; default to discharging if it NAKs.
```

