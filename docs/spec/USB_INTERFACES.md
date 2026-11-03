# USB interfaces for modules

A module can drive a USB function the kernel has no driver for: a camera
today, a microphone next. The kernel keeps the device: it enumerates it,
sets its configuration and moves the data; the module only sees the
interfaces it claimed. Code: `kernel/src/drivers/xhci/usbfn.rs`, host calls
in `kernel/src/wasm/host_core.rs`, SDK wrappers `npk_sys::usb_*`.

## Rights

| Class | Right | `.npk.caps` |
|---|---|---|
| video (0x0E) | `CAMERA` | second byte, bit `0x08` |
| audio (0x01) | `MIC` | second byte, bit `0x10` |

Other classes have no module path. No default grants either right.

While any stream on a video interface is open, the kernel shows the status
item `kernel/camera` (state Attention). It is built from the kernel's own
stream count, so no module can open the camera without it.

## Calls

Errors for every call: -1 gone (unplugged, not yours), -2 nothing free of
that class, -3 denied, -4 the device refused, -5 bandwidth, -6 memory,
-7 bad argument.

- `npk_usb_claim(class, buf, max) -> handle`: claims every interface of
  `class` on the first device that has one and no kernel driver. `buf`
  receives the 18-byte device descriptor followed by the configuration
  descriptor. A device not configured yet is configured first: the
  endpoints of every interface's setting 0, then SET_CONFIGURATION (Linux
  `usb_set_configuration`).
- `npk_usb_control(handle, setup, data, len) -> bytes`: `setup` is the
  8-byte SETUP packet. Allowed: recipient interface with the index of a
  claimed interface (not SET_INTERFACE), recipient endpoint with an
  endpoint of a claimed interface, and GET_DESCRIPTOR to the device.
  Everything else is denied.
- `npk_usb_set_alt(handle, ifnum, alt)`: bandwidth first, then
  SET_INTERFACE (Linux `usb_set_interface`). -5 means the controller has no
  room for this setting; pick a smaller one. Streams on the interface end.
- `npk_usb_stream_open(handle, ep, td_len, count) -> stream`: an IN stream
  on a bulk or isochronous endpoint of the current setting, with `count`
  buffers of `td_len` bytes (0 = one service interval of an isochronous
  endpoint). Up to a ring's worth (254) is queued at once; a buffer that
  comes back is queued again from the interrupt while the module reads
  the ones before it. At most 4096 buffers, 64 KiB each, 32 MiB a stream.
  Isochronous TDs are scheduled as soon as possible (SIA).
- `npk_usb_stream_read(handle, stream, buf, max) -> bytes`: what came
  back, in order, as records of `len: u16, status: u8, 0` and `len` bytes.
  Status 0 ok, 1 missed (an interval the controller skipped), 2 error, 3
  overrun. A record is a TD: one service interval (isochronous) or one
  bulk transfer, shorter than `td_len` when the device ended it.
- `npk_usb_stream_close(handle, stream)`, `npk_usb_release(handle)`: a
  release ends the streams and puts the interfaces back to setting 0; the
  device stays configured. Both happen by themselves when the module ends.
