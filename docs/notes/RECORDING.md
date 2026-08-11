# RECORDING.md — Screen Recordings of the QEMU Guest

How to produce a demo video of nopeekOS that stays sharp: one guest pixel on
one video pixel, no resampling anywhere in the chain. Small UI text is the
whole difficulty — a single unnecessary scaling step turns it to mush, and it
is not recoverable afterwards.

## 1. The guest must render 1:1

`./build.sh boot` (and every other GUI mode) runs the framebuffer at a fixed
1920x1080 and passes:

```
-display gtk,show-menubar=off,zoom-to-fit=off
```

`zoom-to-fit` is **on** by default in QEMU's GTK frontend: it rescales the
framebuffer to whatever size the window happens to have. Under a tiling WM
the window is never exactly 1920x1080, so every frame goes through a resample.
`show-menubar=off` drops the GTK menu strip, so the window *is* the
framebuffer and there is nothing to crop out later.

Override via `QEMU_GTK` if you need the old behaviour:

```
QEMU_GTK=zoom-to-fit=on ./build.sh boot     # scale to window again
QEMU_GTK=full-screen=on ./build.sh boot     # fullscreen demo
```

**On a tiling WM the QEMU window must float.** With `zoom-to-fit=off` a
window smaller than the framebuffer makes QEMU crop instead of scale. Add a
rule that floats the window and pins it to 1920x1080, and disable rounded
corners for it — rounded corners end up in the recording as clipped corners.
For Hyprland:

```
windowrule {
    name = qemu-capture
    match:class = ^(qemu)$
    float = true
    size = 1920 1080
    center = true
    rounding = 0
}
```

Window rules only apply when a window opens — restart the VM after adding it.

## 2. Capture

Use a **window capture** source over the xdg-desktop-portal / PipeWire path
(OBS Studio or Kooha). Window capture takes the window content straight from
the compositor, so it does not matter whether the recorder is focused, in
front of the VM, or being clicked — none of that lands in the video.

Settings that actually matter:

| Setting | Value | Why |
|---|---|---|
| Canvas = output resolution | 1920x1080 | must equal the framebuffer, or the recorder rescales |
| FPS | 30 | plenty for a UI demo, keeps files small |
| Encoder | VAAPI H.264 (AMD/Intel) | near-zero CPU cost — QEMU needs the cores |
| Rate control | CQP, qp 18–20 | constant quality; text needs the low end |
| Container | mkv | survives a crash; remux to mp4 afterwards |

If you plan to add a voice-over later, record desktop audio and mic on
**separate audio tracks** — replacing a mixed-down track means recording
again.

### Running a take

1. Start the VM first, so the window exists and has its final size.
2. Point the window-capture source at it. The portal shows a picker and the
   choice has to come from a human — that consent step cannot be automated,
   and the selection is forgotten whenever the source's restore token is
   cleared, so expect to re-pick after config changes.
3. Click start/stop in the recorder UI. Do **not** rely on global hotkeys:
   on Wayland the recorder cannot grab keys while another window is focused,
   so a hotkey only fires while the recorder itself has focus — which is
   never the case while you are driving the VM.
4. Clicking is harmless here. Window capture only sees the QEMU window, so
   the recorder's own UI, the taskbar, and every click outside the VM stay
   out of the video. Record generously and trim afterwards.

### Known trap: wf-recorder

`wf-recorder` (0.6.0) does not work on Hyprland ≥ 0.55: it sets up the encoder,
writes exactly one frame, then blocks forever and stops responding to SIGINT,
so it has to be SIGKILLed and the mp4 is left without a moov atom. This is
independent of region vs. fullscreen capture, `--no-dmabuf`, `--no-damage`,
and the encoder choice. `grim` still produces correct single frames, so
wlr-screencopy itself is fine. Use the portal/PipeWire path instead.

## 3. Post-processing

Trim first, then encode once per target. Encoding from the untrimmed master
every time wastes minutes on long recordings.

**Repo/README version** — narrow, small enough for GitHub's 10 MB limit:

```bash
ffmpeg -ss 0.5 -to 37 -i rec.mkv \
  -vf "scale='min(1280,iw)':-2:flags=lanczos" \
  -c:v libx264 -crf 23 -preset slow -pix_fmt yuv420p \
  -movflags +faststart -c:a aac -b:a 128k nopeek-demo.mp4
```

**YouTube master** — no scaling, the recording already is the master; crf 18
leaves headroom for YouTube's own re-encode:

```bash
ffmpeg -ss 0.5 -to 37 -i rec.mkv \
  -c:v libx264 -crf 18 -preset slow -pix_fmt yuv420p \
  -movflags +faststart -c:a aac -b:a 192k nopeek-demo-1080p.mp4
```

**GIF** — keep it to 10–15 s of the liveliest part, two-pass palette:

```bash
F="fps=12,scale='min(800,iw)':-1:flags=lanczos"
ffmpeg -ss 3 -to 15 -i rec.mkv -vf "$F,palettegen=stats_mode=diff" pal.png
ffmpeg -ss 3 -to 15 -i rec.mkv -i pal.png \
  -lavfi "$F [x]; [x][1:v] paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle" \
  -loop 0 nopeek-demo.gif && rm pal.png
```

## 4. Publishing

- **GIF** — commit it (e.g. `docs/nopeek-demo.gif`) and embed with
  `![demo](docs/nopeek-demo.gif)`. Works everywhere, including mirrors.
  Keep it under ~5 MB.
- **MP4** — do *not* commit. Drag the file into a GitHub issue comment, take
  the resulting `user-attachments` URL and put it bare on its own line in the
  README; GitHub renders a player. Upload limit is 10 MB.
- **YouTube** — upload the 1080p master. 1080p is effectively the floor:
  below it YouTube assigns worse codecs and text frays after re-encoding.

## Sizes to expect

A 36 s 1080p capture of the desktop compresses to roughly 1 MB — the UI is
mostly static, so inter-frame compression has an easy job. If a file comes out
far larger, something is generating constant motion (an animated wallpaper, a
busy log stream) and is worth checking before you blame the encoder.
