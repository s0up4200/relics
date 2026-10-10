# Prototype: player controls

This is throwaway code for the ticket "Player controls" (s0up4200/silo-omarchy#8). Do not merge it.

One program plays a local test video in two variants:

- `osc` uses the built-in mpv on-screen controller. The skip prompt is OSD text, and Enter takes it.
- `lua` loads `silo-controls.lua` and turns off the built-in controller. The script draws a bar in the colors of the live Omarchy theme. It shows the marker ranges on the seek bar and draws a skip button. You can click the button or press Enter.
- `SKIP_MODE=always` skips at once and offers "Watch intro" to go back. `SKIP_MODE=never` shows no prompt.

In both variants, the app owns the skip logic. The controller only draws the prompt and sends the message `silo-action` back to the app.

The sample has two audio tracks (English at 440 Hz, Norsk at 880 Hz) and two subtitle tracks. The fake markers are a recap from 3 to 8 s, an intro from 15 to 30 s, and credits from 105 to 120 s. The credits run to the end of the file, so the app offers "Next episode" and does not seek.

## Run

Run these commands in `prototypes/player-controls`:

```sh
ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=30 -f lavfi -i sine=frequency=440 \
  -f lavfi -i sine=frequency=880 -t 120 -map 0 -map 1 -map 2 -c:v libx264 \
  -preset ultrafast -c:a aac -metadata:s:a:0 language=eng -metadata:s:a:0 title=English \
  -metadata:s:a:1 language=nor -metadata:s:a:1 title=Norsk sample.mkv
# Also write two SRT files: sample.eng.srt and sample.nor.srt.
cargo run -- osc
cargo run -- lua
SKIP_MODE=always cargo run -- lua
```

In the `lua` variant, Space pauses, Left and Right seek, `a` cycles audio, `s` cycles subtitles, and Enter takes the prompt. In the `osc` variant, the default mpv keys apply (`#` for audio, `j` for subtitles), and Enter takes the prompt.
