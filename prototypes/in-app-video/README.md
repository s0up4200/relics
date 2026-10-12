# Prototype: video in a child surface under the GPUI window

This is throwaway code for the ticket "Prototype: video in a child surface under the GPUI window" (s0up4200/silo-omarchy#44). Do not merge it.

One GPUI window has a transparent video area. mpv draws into a Wayland subsurface below the GPUI surface, through the libmpv OpenGL render API with hardware decoding. A GPUI button over the video pauses and plays. The result and the run steps are in `docs/research/in-app-video-prototype.md`.

## Run

Run these commands in `prototypes/in-app-video`:

```sh
CARGO_TARGET_DIR=$HOME/.cache/silo-omarchy-target nice -n 19 cargo run -j 4 -- /path/to/video.mkv
```

Keys: `f` toggles fullscreen, Space pauses, `q` quits. The terminal shows mpv statistics every 2 s.

These environment variables help tests:

- `TOGGLE_FULLSCREEN=1` goes fullscreen and back every 2 s.
- `OPAQUE_BUFFER=1` uses a buffer with no alpha channel, to show the Hyprland white-video bug.
- `cargo run --bin click -- <x> <y> <back x> <back y> <layout width> <layout height>` clicks at a layout position with a virtual pointer and moves the pointer back.

To make a 10 s 4K test file:

```sh
ffmpeg -vaapi_device /dev/dri/renderD128 -f lavfi -i testsrc2=size=3840x2160:rate=30 -t 10 \
  -vf format=nv12,hwupload -c:v hevc_vaapi -b:v 20M sample-4k.mkv
```
