# CAMERA_MIC.md — camera and microphone, in nopeek and in the guest

Status: **paper**, nothing built (2026-10-20). Goal: video calls in
Firefox in the microVM, with the notebook's camera and microphone; both
usable by nopeek apps too.

## What there is (facts, IdeaPad, kernel 0.663.0)

From `usb` / `usb 1` on the device:

| Device | Where | What |
|---|---|---|
| `30c9:0030` Integrated Camera (Luxvisions) | xHCI 04:00.3 port 4, high speed | UVC 1.10, video only (no audio interface) |
| `27c6:55b4` Goodix fingerprint | 04:00.4 port 3 | vendor specific; later |
| `0bda:c123` Realtek Bluetooth | 04:00.4 port 4 | the BT half of the RTL8822CE; later |

The camera:

- **VideoControl** (interface 0): camera terminal 1 → processing unit 2 →
  extension units 4, 6 → output terminal 3; status by interrupt EP `0x83`
  (32 bytes, 4 ms).
- **VideoStreaming** (interface 1), two formats, ten sizes each:
  - **MJPEG**: every size at 30 fps, up to 1920x1080.
  - **YUY2**: 30 fps up to 640x480, then 848x480@20, 960x540@15,
    1280x720@10, 1920x1080@3.
- **Isochronous EP `0x81` IN**, bandwidth by alternate setting (per 125 us
  microframe): alt 1 128 B · 2 512 · 3 1024 · 4 2x768 · 5 2x1024 ·
  6 3x896 · 7 3x1020 B = 24.5 MB/s. Alt 0 has no endpoint (idle).

So a call at 720p30 is MJPEG, and so is 1080p30. Raw YUY2 is only for
small sizes.

Audio: output works (`audio_hda`, HDA codec on 04:00.6). **There is no
input path anywhere**: not in `audio_hda`, not in `crate::audio`, and
virtio-snd in the microVM answers one OUTPUT stream (`virtio_snd_pci.rs`,
rxq is a stub). The camera brings no microphone, so the built-in mic is
either on the HDA codec (an internal-mic pin) or a PDM microphone on the
ACP (04:00.5 `1022:15e2`, Linux `snd-rn-pci-acp3x` + `acp-pdm-mach`). Which
one is not known yet; step M0 finds out.

What the xHCI driver lacks: **isochronous transfers**. It has control,
interrupt and bulk.

## Principles

- **Camera and microphone are capabilities of their own** (`CAMERA`,
  `MIC`), never part of a default. A module without them cannot open the
  device, and the guest gets them only when the user started it with them.
- **Visible while on.** While a camera or microphone stream is open, the
  kernel publishes a status item (`kernel/camera`, `kernel/mic`,
  `STATUS_ITEMS.md`) with state `Attention`. A publisher cannot hide it:
  it comes from the kernel's own stream count, not from the module.
- **Linux 1:1** for the hardware: `xhci-ring.c` (isoch TDs), `uvcvideo`
  (probe/commit, payload headers), `hda_codec` / `snd-rn-pci-acp3x`.

## Plan

### Microphone

- **M0 — measure.** `audio_hda` prints every pin's default configuration
  (`AC_VERB_GET_CONFIG_DEFAULT`: device, location, connectivity) and the
  ADC widgets; `dsdt` is searched for the ACP's PDM device. Result: where
  the internal mic is, and whether the headset jack has a mic pin.
- **M1 — capture on HDA.** Input stream descriptor (input streams come
  first in the SD array, `regs.rs` already says so), ADC + input amp +
  pin as input (Linux `hda_generic` capture path), a capture ring in the
  kernel mailbox mirroring the playback one. If M0 says the internal mic is
  PDM, M1 is still the headset mic and M2 adds the ACP.
- **M2 — ACP PDM** (only if M0 says so): its own module after
  `snd-rn-pci-acp3x` / `acp3x-pdm-dma`.
- **M3 — guest.** virtio-snd announces a second stream, `D_INPUT`, and
  serves `rxq` from the capture ring (virtio 1.2 §5.14). PipeWire in the
  guest sees a source; Firefox asks for it like on any Linux.

### Camera

- **C1 — isochronous transfers in xHCI** (`xhci-ring.c`
  `xhci_queue_isoc_tx`, `xhci_get_isoc_frame_id`): an isoch endpoint
  context (Mult, Max Burst, ESIT payload), one TD per service interval,
  completion per TD with its actual length; the ring kept full (UVC has
  no retry: a missed microframe is a lost slice of a frame). Bandwidth by
  Configure Endpoint, as Linux does: alt setting chosen by what the
  stream needs, not the largest.
- **C2 — a USB interface handed to a module.** Today USB drivers live in
  the kernel (HID, the r8152 NIC). A camera driver does not need to: it
  needs control transfers on its interface and an isoch stream. A small
  host API — claim interface (by class, gated by the matching capability:
  video → `CAMERA`), control transfer on it, set alternate setting, read
  isoch completions — makes UVC a module, and later Goodix and Bluetooth
  too. Only claimed interfaces are reachable; EP0 requests are filtered
  to the interface's own (recipient interface/endpoint), so a module
  cannot re-address or reconfigure the device.
- **C3 — `uvc` module** after `uvcvideo`: probe/commit
  (`VS_PROBE_CONTROL`, `VS_COMMIT_CONTROL`), payload header parsing (FID
  toggles a frame, EOF ends it, error bit), frames out as MJPEG or YUY2.
  A small viewer shows it (MJPEG needs a decoder in nopeek; the guest
  decodes itself).
- **C4 — guest.** Open. Firefox on Linux takes the camera from V4L2. Three
  ways to get one into the guest, none decided:
  1. **Emulated USB in the VMM** (an xHCI with a UVC device behind it, fed
     from C3's frames, MJPEG passed through): the guest's own `uvcvideo`
     drives it, nothing to install in the guest. Most work in the VMM.
  2. **Forward the real device** (usbredir-style: the guest's URBs go to
     the real camera through C2). Less code, but the guest drives the
     hardware and nopeek cannot use the camera meanwhile.
  3. **A virtio video device** (virtio-video / virtio-media): smallest
     device model, but needs a guest driver; whether one is in the guest
     kernel we ship has to be checked first.

  The choice depends on how far C2/C3 carry; (1) keeps the hardware in
  nopeek's hands, which the principles above want.

### Order

M0 → M1 → M3 (a mic in calls, small), then C1 → C2 → C3 → C4. Each step
is testable alone: M1 with a recording intent, C1 with a byte counter on
the isoch stream, C3 with the viewer.

## Security checkpoint

- New rights `CAMERA` and `MIC`; nothing gets them by default.
- C2 hands a module one interface, not the device: no SET_ADDRESS, no
  SET_CONFIGURATION, no requests to other interfaces.
- The status item for an open stream is the kernel's, so a module cannot
  record unseen.
