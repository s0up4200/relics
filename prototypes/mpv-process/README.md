# Prototype: mpv inside the app, or as a separate process?

This is throwaway code for the ticket "mpv inside the app, or as a separate
process?" (s0up4200/silo-omarchy#2). Do not merge it.

The directory has three programs:

- `inproc` runs libmpv inside the app process, with the crate `libmpv2` 6.0.0.
- `child` starts mpv as a child process and controls it over JSON IPC on a socketpair.
- `fake_silo.py` is a local server on 127.0.0.1:47613. It serves `sample.mp4` only to a request with `Authorization: Bearer proto-token`.

## Run

Run these commands in `prototypes/mpv-process`:

```sh
ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=30 -f lavfi -i sine -t 120 \
  -c:v libx264 -c:a aac -movflags +faststart sample.mp4
python3 fake_silo.py &
cargo run --bin inproc -- http://127.0.0.1:47613/stream/proto
cargo run --bin child  -- http://127.0.0.1:47613/stream/proto
```

To send a different token, set `SILO_TOKEN`. To run with no window and no
sound, set `PROTO_HEADLESS=1`.
