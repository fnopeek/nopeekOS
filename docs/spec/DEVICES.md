# Device register

What the machine has of each media kind, and the one of each kind that is
active. Apps and the microVM never pick a device themselves: they use the
active one, and the user switches it in one place. Code:
`kernel/src/devices.rs`, host calls in `kernel/src/wasm/host_core.rs`, SDK
`npk_sys::device_*` and `nopeek_widgets::host::devices`.

## Kinds

| Kind | ABI | Entered by |
|---|---|---|
| audio out | 0 | audio drivers (`audio_hda`) |
| audio in | 1 | audio drivers |
| camera | 2 | the kernel, for each USB device with a video interface and no kernel driver |

An entry has a handle, an id that stays the same across boots and replugs
(bus path and identity, for a camera `usb-<controller>-<port path>-<vid>:<pid>`),
a name for the user and a rank. The driver that entered it takes it out
when the device goes; a module's entries go when the module ends.

## Which one is active

1. The one the user picked, kept by id under `devices.audio_out`,
   `devices.audio_in` or `devices.camera` in the config. A picked device
   that comes back is active again.
2. Otherwise the highest rank, the earliest entered among equals:

| Rank | What |
|---|---|
| 3 | a plugged jack or headset |
| 2 | a USB device |
| 1 | built in |
| 0 | a display's audio (HDMI, DisplayPort) |

## What follows from it

- **Audio out.** The mixer (`audio::poll_mix`) plays to the driver whose
  entry is active. Every other audio driver gets silence and takes nothing
  from the streams, so switching moves playing streams without a gap in
  their data. A driver that entered nothing plays only while no output is
  active.
- **Camera.** `npk_usb_claim` for the video class claims the active camera
  only (`USB_INTERFACES.md`). The register never opens a device: a camera
  runs only while someone streams from it, and the kernel's `camera`
  status item shows that.
- **microVM.** The guest gets the active device of each kind and keeps
  seeing one speaker, one microphone and one camera when the user
  switches.

## Calls

- `npk_device_register(kind, rank, id, id_len, name, name_len) -> handle`
  or -1: audio out and in only, needs `HARDWARE`. Id up to 64 bytes,
  unique per kind; name up to 64 bytes; no control characters. 16 entries
  per module.
- `npk_device_remove(handle) -> 0`, `npk_device_active(handle) -> 1 / 0`:
  the entering module's own entries only, else -1.
- `npk_device_list(kind, buf, max) -> bytes` or `-(needed + 2)`, needs
  `SHELL`. Records `handle u32, flags u8 (bit 0 active), id_len u8,
  name_len u8, 0`, then the id and the name.
- `npk_device_select(handle) -> 0`, needs `SHELL`: the user's pick.

Every change (an entry comes or goes, a pick) is the notify topic
`TOPIC_DEVICES`, so a module waiting with `WAIT_STATE` sees it at once.
