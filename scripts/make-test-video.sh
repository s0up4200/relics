#!/bin/sh
# Makes the test video of the mpv adapter tests: 6 seconds of 320x180 H.264 video
# with an AAC tone, as one MP4 and as HLS with three 2-second segments.
# The output goes to tests/media/. Commit it.
set -eu
out="$(dirname "$0")/../tests/media"
mkdir -p "$out/hls"
rm -f "$out"/sample.mp4 "$out"/hls/*
ffmpeg -hide_banner -loglevel error \
    -f lavfi -i testsrc=size=320x180:rate=25 -f lavfi -i sine=frequency=440 -t 6 \
    -c:v libx264 -preset veryslow -crf 35 -g 50 -keyint_min 50 -sc_threshold 0 -pix_fmt yuv420p \
    -c:a aac -b:a 32k -ac 1 -movflags +faststart -map_metadata -1 -fflags +bitexact \
    "$out/sample.mp4"
ffmpeg -hide_banner -loglevel error -i "$out/sample.mp4" -c copy \
    -f hls -hls_time 2 -hls_list_size 0 -hls_playlist_type vod \
    -hls_segment_filename "$out/hls/segment%d.ts" "$out/hls/index.m3u8"
du -ch "$out"/sample.mp4 "$out"/hls/* | tail -n 1
