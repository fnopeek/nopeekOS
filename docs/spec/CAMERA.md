# Camera

The camera has one owner, the `uvc` driver module, and any number of
readers of what it records. Code: `kernel/src/camfeed.rs` (the feed),
`tools/wasm/uvc` (the driver, after Linux `uvcvideo`), `tools/wasm/cam`
(a viewer), the microVM's port in
`kernel/src/microvm/devices/virtio_console_pci.rs` and the guest's
`microvm/linux/npkcam`.

## On only while someone watches

`uvc` waits until a reader exists, then claims the active camera of the
device register (`DEVICES.md`) through `npk_usb_*` (`USB_INTERFACES.md`),
negotiates the largest size a reader asked for and streams. When the last
reader closes it stops the stream and lets the camera go. While it
streams the kernel shows the status item `kernel/camera`, counted from its
own USB streams.

## The feed

The feed holds the newest frame as the camera sent it: MJPEG, or YUY2
(BT.601 limited) for small sizes, with its size and a sequence number.
Nothing queues: a reader takes the newest frame it has not seen, a slow
reader skips frames instead of lagging behind.

| Call | Who | Contract |
|---|---|---|
| `npk_camera_wanted() -> readers \| width << 16 \| height << 32` | CAMERA | How many read, and the largest size one asked for. |
| `npk_camera_push(ptr, len, width, height, format) -> 0 / -1` | CAMERA, holding the active camera | A frame; format 1 YUY2, 3 MJPEG; up to 8 MiB. Any other module is refused, so none can show a picture as the camera. |
| `npk_camera_open(width, height) -> reader` | CAMERA | A reader wanting that size; 4 per module. |
| `npk_camera_read(reader, buf, max, info) -> bytes` | the reader's module | The newest unseen frame, 0 for none, `-(needed + 2)` when it does not fit (the frame stays unread). `info`: width u16, height u16, format u8, 3 pad, seq u64. |
| `npk_camera_close(reader) -> 0 / -1` | the reader's module | A module's readers close when it ends. |

When the driver lets the camera go, its last frame stops counting as new.

## In the microVM

The guest runs `npk-camera`, a PipeWire video source (media.role
Camera), which xdg-desktop-portal's Camera portal offers to the browser.
When an app streams from it, it sends `CAM_START` (width, height) on the
virtio-console port `npk.camera`; the host opens a feed reader and sends
each new frame as `u32 len | u8 0x10 | u16 width | u16 height | u8 format |
3 pad | data`, the newest only. `CAM_STOP`, a device reset and the end of
the VM close the reader. The guest decodes MJPEG to YUY2. The browser asks
the user before a page gets the camera; nopeek itself does not ask yet.
