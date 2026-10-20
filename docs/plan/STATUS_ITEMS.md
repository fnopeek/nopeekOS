# STATUS_ITEMS.md — the panel shows, the tools act

Status: **plan** (2026-10-20). Nothing built.

## Why

deck's right side knows two sources by name: `npk_battery` and
`npk_audio_get_volume`, each with its own icons, thresholds and click
target in deck's code. The notebook now needs WLAN, later Bluetooth and
brightness. Doing each the same way makes deck grow with every device and,
worse, makes it the place where the work happens: scanning, connecting and
keeping a WLAN password need NET and the password, Bluetooth pairing needs
its own rights. A panel holding the union of all of them breaks principle 1
(capabilities, not permissions).

So deck only **shows**. Whoever has something to report publishes a small,
plain-data entry; a click opens the tool that owns the work, with that
tool's own rights.

## What there is today (facts, 2026-10-20)

| Piece | Today | Where |
|---|---|---|
| Wake on change | 4 fixed topic bits (windows, battery, volume, config), raised by the kernel only; subscribing needs RENDER; the waiter learns *that* something changed, not *what* | `kernel/src/notify.rs`, `host_core.rs` `npk_wait` |
| Battery | `npk_battery` (RENDER), fed by aml via `npk_battery_report` | `drivers/battery.rs` |
| Volume | `npk_audio_get_volume` (no gate) | `audio.rs` |
| deck readouts | hardcoded icon/threshold per source; only volume clickable | `deck/src/lib.rs` `volume_readout`, `battery_readout` |
| Opening a tool from deck | `npk_launch("volume")`: no anchor, no dedup | `host_core.rs` `npk_launch` |
| volume's position | hardcoded top right, `y = 48` (from when the bar was on top) | `volume/src/lib.rs` |
| Popups | own window only (`npk_popup` anchors in the caller's tree) | `host_core.rs` `npk_popup` |
| Module → module | none generic; `npk_open` pushes `Event::Open(arg)` | `host_core.rs` |

## The model

### 1. A status item

Plain data, owned by one publisher:

| Field | Type | Meaning |
|---|---|---|
| owner | module name (set by the kernel, not the caller) | who published it; the kernel for its own items (`kernel`) |
| key | ≤ 16 bytes | the publisher's name for it (`wlan`, `battery`) |
| icon | `IconId` | what to draw |
| value | i32 | a number the item is about (percent, signal 0..4), −1 = none |
| text | ≤ 48 bytes UTF-8 | short label / tooltip (`FRITZ!Box 7590`, `86 %, charging`) |
| state | `Normal` / `Attention` / `Off` | colour: normal, attention (accent / danger), dimmed |
| place | `Bar` / `Quick` | right side of the bar, or only in the quick-settings popup |
| opens | module name | the tool a click opens (may be the owner itself) |

Limits: 4 items per owner, 32 in all; a publisher that ends has its items
removed (same hook as `notify::forget`).

The **owner is set by the kernel** from the calling module, never taken
from the caller. deck shows it in the tooltip, and the order on the bar is
fixed by owner + key (below), so a module cannot pass itself off as the
WLAN entry by choosing the right icon.

### 2. Kernel registry + topic

`kernel/src/status.rs`: the table above, and a fifth topic
`TOPIC_STATUS = 1 << 4` raised on every change. The kernel publishes its
own items through the same table:

- `kernel/battery` — from `battery::report` (replaces the special case).
- `kernel/volume` — from `audio::set_volume`.
- `kernel/net` — wired link from `netdev`, later.

### 3. Host functions (appended to the ABI)

| Call | Right | Does |
|---|---|---|
| `npk_status_set(key, icon, value, state, place, text, opens)` | none beyond being a module | create or replace the caller's item `key` |
| `npk_status_clear(key)` | none | remove it |
| `npk_status_list(buf, max)` | RENDER | the whole table, serialized, for the panel |
| `npk_launch_at(name, arg, anchor_node)` | EXECUTE | `npk_launch`, plus: the new app's first `npk_window_set_overlay` is placed next to `anchor_node` of the caller's window (above it for a bottom panel), light-dismiss on |

Publishing needs no right on purpose: it writes only plain data into a
bounded table, under the caller's own name, and draws nothing. wifid has no
RENDER and must be able to publish.

`npk_launch_at` replaces volume's hardcoded corner. The anchor is resolved
in the **caller's** tree at call time (`widgets::anchor_rect`, as
`npk_popup` does), so the launched app never sees another window's layout.

### 4. What deck shows

- **Bar (right, before the clock):** items with `place = Bar`, in a fixed
  order: `net` / `wlan`, `volume`, `battery`, then any other publisher in
  name order. Nothing for a source that does not exist: no battery item on
  the NUC, no WLAN item without a WLAN driver.
- **Quick settings:** one icon on the bar when at least one item has
  `place = Quick`. It opens deck's own popup (`npk_popup`) with a tile per
  item: icon, text, value. A tile click is `npk_launch_at(opens, key, tile)`.
- A bar item's click is the same call, anchored at the item.
- Context click: `Event::Open("<key>:context")` to `opens` if it runs
  (volume uses it for mute), else nothing.

deck loses `npk_battery`, `audio_volume`, `volume_readout`,
`battery_readout`; it gains one generic readout.

### 5. Tools

- **volume:** placed by `npk_launch_at` instead of `y = 48`; waits with
  `host::wait` instead of polling every 16 ms.
- **wifi (new) and wifid:** own paper, `docs/plan/WIFI_TOOL.md`. Open
  questions it must answer, found while writing this one:
  - wifid never scans or connects itself — the driver picks the AP from
    `sys/config/wifi`; a tool that lists networks needs `SCAN`/`SCAN_AP`
    through wifid (`WIFI_CLASS_ABI.md` §4 has the wire format, unused).
  - There is no path from a UI to wifid at all.
  - The passphrase is plaintext in `sys/config/wifi_psk`, readable by any
    module with READ. It belongs in `priv/wifid/` (owner-only) until a
    keystore exists, and the tool must hand it to wifid, not write it
    where everyone can read it.
  - wifid publishes `wlan` (connected SSID, signal 0..4) through
    `npk_status_set` — that part needs nothing else.
- **brightness, Bluetooth:** a status item + a small tool each, when the
  drivers exist.

## Steps

1. `status.rs`, `TOPIC_STATUS`, the three status calls; kernel battery and
   volume publish through it.
2. deck renders the list; battery/volume special cases go.
3. `npk_launch_at`; volume uses it.
4. Quick-settings popup in deck.
5. `WIFI_TOOL.md`, then wifid publishes `wlan`, then the tool.

Steps 1–3 change nothing visible except volume opening at the bar — that is
the proof the path carries before WLAN depends on it.

## Security checkpoint

- Publishing writes ≤ 4 × ~100 bytes per module into a kernel table, under
  a name the kernel sets: no spoofing of the owner, bounded memory.
- Reading the list needs RENDER (it reveals e.g. the SSID).
- `npk_launch_at` grants nothing new: the launched app gets its own
  `.npk.caps` as with `npk_launch`; the anchor is a rectangle of the
  caller's own window.
