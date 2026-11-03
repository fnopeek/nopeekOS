# Audio

The kernel's audio mailbox and mixer (`kernel/src/audio.rs`) and the host
calls around it, and the capture side. The kernel knows no audio
hardware; driver modules (`audio_hda`, `audio_acp`) pull the mixed stream
or push what they record.

## Streams (sources)

| Call | Contract |
|---|---|
| `npk_audio_open() -> slot` | A new stream of the caller, S16LE, 48 kHz, stereo. -1 when the caller already holds 8 streams (a safety cap, logged). The number of streams is otherwise unbounded. |
| `npk_audio_format(slot, rate, encoding, channels) -> 0 / -1` | The stream's format from now on; empties its ring. `encoding`: 0 = S16LE, 1 = S32LE, 2 = F32LE (full scale is ±1.0). `rate`: 8000 to 384000 Hz. `channels`: 1 or 2; mono plays on both front channels. |
| `npk_audio_submit(slot, ptr, len) -> bytes` | Appends interleaved PCM in the stream's format. Takes fewer bytes when the ring is full; the caller retries. The ring holds about 1.36 s of the stream's format. |
| `npk_audio_buffered(slot) -> bytes` | Bytes not yet mixed: the play clock. |
| `npk_audio_close(slot) -> 0 / -1` | Ends the stream and frees its ring. Streams of a module that ends are closed for it. |

Only the module that opened a stream may use it.

## Device (driver)

| Call | Contract |
|---|---|
| `npk_audio_device_format(rate, encoding, channels) -> 0 / -1` | What `npk_audio_poll_mix` produces from now on; S16LE, 48 kHz, stereo until set. `channels` 1 to 8. Needs `Rights::HARDWARE`. |
| `npk_audio_poll_mix(ptr, max) -> bytes` | Fills whole frames of the device format: every stream converted to it and summed, scaled by the master volume, clamped. Silence where no stream has data. Needs `Rights::HARDWARE`. |

Conversion: samples become full-scale 32-bit, then the rate is converted by
linear interpolation between neighbouring source frames (as ALSA's
`linear` rate plugin). A stereo source fills device channels 0 and 1, the
others stay silent; a mono device gets the average of left and right. A
device encoding wider than 16 bits gets the sample in the most significant
bits (HDA's 20 and 24 bit containers).

Each driver has its own device format. Only the driver whose output is
the active one of the device register (`DEVICES.md`) plays the mix; the
others get silence and take nothing from the streams.

## Capture

`kernel/src/capture.rs`: the driver of the active audio input pushes what
it records, every open capture stream gets its own copy in its own format
(the conversion above, in the other direction). A stream that is not read
loses its oldest bytes when its ring (about 1.4 s) is full.

| Call | Contract |
|---|---|
| `npk_audio_capture_open() -> stream` | A capture stream of the caller, S16LE, 48 kHz, stereo. Needs `Rights::MIC`. -1 when refused or the caller holds 4. |
| `npk_audio_capture_format(stream, rate, encoding, channels) -> 0 / -1` | As `npk_audio_format`; empties the ring. |
| `npk_audio_capture_read(stream, ptr, max) -> bytes` | Whole frames recorded since the last read, up to `max`. |
| `npk_audio_capture_close(stream) -> 0 / -1` | Ends the stream; a module's streams end with it. |
| `npk_audio_capture_device_format(rate, encoding, channels) -> 0 / -1` | Driver: what it pushes, mono or stereo. Needs `Rights::HARDWARE`. |
| `npk_audio_capture_wanted() -> streams` | Driver: how many streams listen while its input is the active one, else 0. At 0 the driver stops its hardware: the microphone records only while someone listens. |
| `npk_audio_capture_push(ptr, len) -> bytes` | Driver: recorded whole frames to every stream; 0 while its input is not the active one. |

While any capture stream is open, the kernel shows the status item
`kernel/mic` (state Attention), counted from its own streams. `mic [s]` in
the terminal records from the active input and plays it back.

## ACP driver

`audio_acp` drives the digital microphone of AMD Renoir's audio
coprocessor (`1022:15e2` revision 1) after Linux `snd-rn-pci-acp3x` and
`acp3x-pdm-dma`: present when the ACP's ACPI node answers `_WOV` with
non-zero, 48 kHz, S32LE, stereo. The DMI list of machines whose `_WOV` is
wrong is not applied (no SMBIOS reader yet).

## HDA driver

`audio_hda` probes every codec on the link (STATESTS), parses each codec's
pins and widget graph as Linux `snd-hda-codec-generic` does, and drives
the first codec with an analog output, else the first with any output
(Intel display codecs through `intelhdmi.rs`). The device format is the
best the chosen converters take: 48 kHz if supported, 16 bits if
supported, stereo unless the converter is mono.

A codec with a fixed microphone pin (`AC_JACK_MIC_IN`, connectivity
fixed) gets a capture path as `hda_generic` builds it: the first analog
input converter the pin reaches, the pin as input with its default VREF,
the path's amps opened, the converter's capture volume at its top. It is
entered as "Built-in microphone" and
records on the first input stream descriptor, in the converter's best
format, only while the kernel says a capture stream listens. The headset
microphone on the jack is not set up yet.

## Display audio (component)

What Linux' audio component carries between the display driver and the
audio driver. Every call needs `Rights::HARDWARE`; the display side is the
driver of a PCI class 03 device, the audio side of a class 04 device.

| Call | Side | Contract |
|---|---|---|
| `npk_display_audio_set(port, ptr, len) -> 0 / -1` | display | The ELD of `port`'s sink, `len` 0 for none (`pin_eld_notify`). |
| `npk_display_audio_get(port, ptr, max) -> len` | audio | The ELD of `port`, 0 for no sink, `-(needed + 2)` for a short buffer (`get_eld`). |
| `npk_display_audio_generation() -> n` | both | 0 until the display side reported once, then a count that moves with every report. |
| `npk_display_audio_serve(done) -> want` | display | Binds the component and says request `done` is carried out; returns `(number << 2) \| bits` of what is wanted now. Bit 0: hold the display's audio power (`get_power` / `put_power`). Bit 1: force the codec wake signal (`codec_wake_override`). The display driver calls it on every pass of its service loop, and again after carrying out a new number. |
| `npk_display_audio_request(bits) -> number` | audio | Asks for `bits` (as above) instead of what was asked before, and wakes the display driver. -2 while no display driver is bound, -3 when the bus has no Intel display device, so none will bind (`i915_gfx_present`); -1 refused. |
| `npk_display_audio_done() -> number` | audio | The last request carried out; -2 while no display driver is bound. |

What an audio module asked for is given back when it ends. When the
display driver ends, nothing it carried out holds; a new one carries out
what is still wanted. `audio_hda` waits up to 60 s for the component to
bind, as `snd_hdac_i915_init` does, then holds the power and forces the
codec wake signal around the controller reset (`hda_intel_init_chip`), so
the display codec is present on the link however the boot went.
