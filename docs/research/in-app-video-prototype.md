# Prototype result: video in a child surface under the GPUI window

Date: 2026-10-12. Ticket: #44 (spec #40). Code: `prototypes/in-app-video` on the branch `prototype/in-app-video`. The code is throwaway. Do not merge it.

## Result

Option C of `docs/research/in-app-video.md` works on this machine. mpv draws a 4K video into a Wayland subsurface (a child surface that the compositor draws as part of the window) under the GPUI window. A GPUI bar and a GPUI button show over the video, and the button gets clicks. The user must still check smoothness and resize behavior on the real monitor, and then give go or no-go.

Test machine: Hyprland 0.56.2, AMD HawkPoint GPU (VA-API), mpv 0.41.0, two monitors at scale 2.

## How the prototype works

1. GPUI opens one window with `WindowBackgroundAppearance::Transparent`.
2. The app reads the `wl_display` and the `wl_surface` of the window from `HasDisplayHandle` and `HasWindowHandle`. It opens the same connection with `Backend::from_foreign_display`.
3. The app makes a child surface and a subsurface in desync mode, and places it below the GPUI surface. The child surface has an empty input region, so the pointer goes through it to GPUI.
4. A render thread owns an EGL context (desktop OpenGL) on a `wl_egl_window` of the child surface. It creates the mpv render context with `MPV_RENDER_PARAM_WL_DISPLAY`. On each mpv update, it calls `render`, `eglSwapBuffers` and `report_swap`.
5. The video area in GPUI has no background, so GPUI leaves a transparent hole there. A `canvas` in the video area reads its bounds in paint. The app then calls `set_position` on the subsurface and sends the new size to the render thread.

## What I verified

I ran the prototype on a temporary headless Hyprland output, so that no test window covered the work of the user. I used a 10 s 4K HEVC test file at 30 fps (`testsrc2`).

- The prototype builds with the shared target dir. Only the prototype crate and a few crates with other features compiled.
- The prototype starts. mpv uses `hwdec=vaapi` on the 4K file.
- Frames reach the child surface. The render thread swapped about 30 frames each second. Screenshots show the video in the hole, with the GPUI bar and the half-transparent button over it.
- In runs of 10 s to 16 s, mpv reported 0 dropped frames, 0 decoder drops and 0 delayed frames. The headless output is not the real monitor, so this result is only a first sign.
- The button gets clicks through the video. A virtual pointer (`src/bin/click.rs`) clicked the button two times. The log showed `button: click 1` and `button: click 2`, the pause state changed, and the hover color showed.
- Resize and fullscreen do not crash. The window went fullscreen and back every 2 s for 16 s (`TOGGLE_FULLSCREEN=1`). A second window opened and closed, and the tiler resized the first window. The video followed the GPUI layout in each screenshot.

## A Hyprland bug that the route must avoid

The video buffer must have an alpha channel. With an opaque buffer format (XRGB, EGL `ALPHA_SIZE` 0), Hyprland 0.56.2 shows plain white in place of the video. This occurs only when the subsurface reaches the bottom-right corner of the window. If the subsurface stops 1 px short of the right edge or the bottom edge, the video shows. A test that drew plain green in place of mpv showed the same white, so mpv is not the cause. I did not find the cause in the Hyprland source.

With an ARGB buffer (`ALPHA_SIZE` 8), the video shows in all positions, also in fullscreen. mpv writes alpha 1, so the video stays opaque. The cost is that Hyprland cannot skip the drawing under the video. `OPAQUE_BUFFER=1` starts the prototype with the opaque buffer, to show the bug.

## What I could not verify

- Smoothness at 4K on the real monitor, with real files: 24 fps film, 10-bit HEVC, 60 fps video, and long playback.
- Misplaced frames during an interactive resize or a fullscreen change. GPUI applies the position of the video with its own commit, but the render thread applies the new size with its next swap. The two can disagree for one frame. Screenshots cannot show one frame.
- What the monitor shows. All screenshots go through screencopy, which draws the scene a second time.
- Keys. I did not send key presses, because the keyboard focus belonged to the work of the user.
- Fractional scale. The prototype sets an integer buffer scale. A fractional scale needs `wp_viewporter`.
- Subtitles, the mpv OSD, HDR video, and audio sync.

## Undocumented GPUI behaviors that the route depends on

All references are to `gpui-pre` 0.3.7 (Zed commit `1a28cff`). A `gpui-pre` update can change each of them.

1. On Wayland, `HasWindowHandle` and `HasDisplayHandle` give the real libwayland `wl_surface` and `wl_display` (`wayland-backend` with `client_system`). `Backend::from_foreign_display` and `ObjectId::from_ptr` depend on this.
2. With `WindowBackgroundAppearance::Transparent`, the wgpu renderer clears each frame to transparent and uses the `PreMultiplied` alpha mode, and GPUI sets no opaque region. An element with no background leaves pixels with alpha 0.
3. GPUI commits its `wl_surface` after paint, in the same frame. So a `set_position` that the app sends from a `canvas` paint callback applies with that GPUI frame.
4. GPUI keeps one `wl_surface` for the life of the window. When the GPU device is lost, GPUI makes a new wgpu surface on the same `wl_surface`. If GPUI made a new `wl_surface`, the video subsurface would lose its parent.
5. The origin of the GPUI layout is the origin of the `wl_surface`. This is true while the app sets no client inset (`set_client_inset`). With a client inset, for example for client-side shadows, the video position must add the inset.
6. GPUI draws with Vulkan, and the video uses EGL and OpenGL in the same process. This worked here, but GPUI does not promise it.

The research note also named one more behavior: GPUI ignores pointer input on a surface that it does not know. Because of the empty input region, the compositor never sends pointer input to the video surface. So the prototype does not depend on this behavior.

## Run steps for the user

Do these steps on the machine with the real monitor. Use a debug build, because a release build compiles all of GPUI again.

1. Go to `prototypes/in-app-video` in a checkout of the branch `prototype/in-app-video`.
2. Run `CARGO_TARGET_DIR=$HOME/.cache/silo-omarchy-target nice -n 19 cargo run -j 4 -- /path/to/a/4k/file.mkv`.
3. Watch the video for one minute and look for stutter. Every 2 s, the terminal shows a `stats:` line. The values `vo-drops`, `decoder-drops` and `delayed` must stay at 0 or near 0, and `hwdec` must not be `no`.
4. Make the window float (Super+T in Omarchy) and drag a corner fast. Look for video that lags behind the hole, shows outside the hole, or leaves a gap under the GPUI bar.
5. Press `f` two times to go fullscreen and back. Look for the same faults during the change.
6. Click the button at the bottom left, and press Space. Each one must pause and play. Press `q` to quit.
7. Optional: put `OPAQUE_BUFFER=1` before the command of step 2. If the video area is white, the Hyprland bug also shows on the real monitor.
8. Give go or no-go on #44. If no-go, revise tickets #45 to #52.
