# Research: video inside the app window, and track lists

Date: 2026-10-12. Status: research only. No code changed.

This research answers two questions. Can the app show video inside the GPUI library window, and not in a separate mpv window? Can the user pick a subtitle or audio track from a list, and not cycle through the tracks?

## Versions in this repo

All findings apply to these pinned versions in `Cargo.lock`:

| Crate | Version | Source |
|---|---|---|
| `gpui-pre` (GPUI) | 0.3.7 | A crates.io snapshot of Zed commit [`1a28cff`](https://github.com/zed-industries/zed/commit/1a28cff4b409169bac058bca40dfbfeb7621d19b) (2026-09-27). Its `Cargo.toml` says "gpui-pre snapshot of zed@1a28cff". |
| `gpui-pre-wgpu` (the renderer) | 0.3.7 | The same Zed commit, crate `gpui_wgpu` |
| `wgpu` | 29.0.4 | [Zed `Cargo.toml`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/Cargo.toml#L937) |
| `wayland-backend` | 0.3.17 | With the `client_system` feature ([gpui_linux `Cargo.toml`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_linux/Cargo.toml#L95-L96)) |
| `libmpv2` | 6.0.0 | [kohsine/libmpv-rs](https://docs.rs/crate/libmpv2/6.0.0) |
| `libmpv2-sys` | 4.0.1 | Ships the headers of mpv client API 2.5 |
| `khronos-egl` | 6.0.0 | Already in the lock, through `wgpu-hal` |
| mpv on this machine | 0.41.0 | From `pacman -Q mpv`. The mpv links below use tag `v0.41.0`. |

## What GPUI can draw on Linux

On Linux, GPUI draws with wgpu, on Vulkan or GL ([wgpu_context.rs L445](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_wgpu/src/wgpu_context.rs#L445)). This version has no blade renderer.

`paint_surface` works only on macOS. `PaintSurface` holds a `CVPixelBuffer`, and that field exists only on macOS ([scene.rs L768-L774](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/scene.rs#L768-L774), [window.rs L4973](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/window.rs#L4973)). The wgpu renderer skips surface batches, with the comment "Surfaces are macOS-only for video playback and are not implemented by the WGPU renderer" ([wgpu_renderer.rs L1612-L1614](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_wgpu/src/wgpu_renderer.rs#L1612-L1614)). Zed `main` has the same code today ([main@93d7456](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui_wgpu/src/wgpu_renderer.rs)).

GPUI has no API for an external texture. App code gets no handle to the wgpu device or queue. The only GPU fact that `Window` gives is `gpu_specs()` ([window.rs L6714](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/window.rs#L6714)). So the app cannot give GPUI a texture that it made.

Images go through the sprite atlas (one large GPU texture that holds many small images). `RenderImage` is BGRA pixel data ([assets.rs L42-L49](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/assets.rs#L42-L49)). `ImageSource::Render(Arc<RenderImage>)` gives it to `img()` ([img.rs L42-L51](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/elements/img.rs#L42-L51)). `paint_image` keeps each image in the atlas by its `ImageId` ([window.rs L4871-L4906](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/window.rs#L4871-L4906)). The atlas copies the bytes to the GPU with `queue.write_texture` ([wgpu_atlas.rs L274](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_wgpu/src/wgpu_atlas.rs#L274)). Each new frame has a new `ImageId`. So the app must call `drop_image` on the old frame ([window.rs L4989](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/window.rs#L4989)), or GPU memory grows without limit.

The app can get the Wayland surface of the window. `Window` implements `HasWindowHandle` and `HasDisplayHandle` ([window.rs L7360-L7371](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui/src/window.rs#L7360-L7371)). On Wayland, the handle is the `wl_surface` pointer of the window ([wayland/window.rs L1621-L1629](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_linux/src/linux/wayland/window.rs#L1621-L1629)). `wayland-backend` uses `client_system`, so these are real libwayland objects. Other code can use the same connection with `Backend::from_foreign_display` ([wayland-backend 0.3.17, sys/client_impl/mod.rs L271](https://docs.rs/crate/wayland-backend/0.3.17/source/src/sys/client_impl/mod.rs#271)). GPUI itself makes no `wl_subsurface`. A search of `gpui_linux` finds none.

GPUI ignores pointer input on a surface that it does not know. On `wl_pointer.enter`, GPUI looks up the surface and does nothing when the surface is not one of its windows ([client.rs L2196](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_linux/src/linux/wayland/client.rs#L2196)).

## What mpv can do

The `wid` option does not work on Wayland. The client API says that `wid` embedding "works on X11, win32, and macOS only" ([client.h L190-L205](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h#L190-L205)). The `--wid` docs list X11, win32 and Android, and not Wayland ([options.rst L3744](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst#L3744)). So mpv cannot draw into a `wl_surface` that the app owns by itself.

The render API (the libmpv API that lets an app draw the video) has two backends, OpenGL and SW ([render.h L50-L54](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/render.h#L50-L54)). It has no Vulkan backend. mpv recommends the render API over `wid` ([render.h L43-L47](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/render.h#L43-L47)).

The render API uses the old `gpu` renderer, not `gpu-next`. The render context calls `gl_video_init` ([libmpv_gpu.c L88](https://github.com/mpv-player/mpv/blob/v0.41.0/video/out/gpu/libmpv_gpu.c#L88)), which is the renderer of `--vo=gpu`. The VO is `libmpv`, which "supports many of the options the `gpu` VO has" ([vo.rst L617-L624](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/vo.rst#L617-L624)). HDR passthrough with `--target-colorspace-hint` "Requires a supporting driver and `--vo=gpu-next`" ([options.rst L7089-L7098](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst#L7089-L7098)). So every option that puts video in the app window loses `gpu-next` and HDR passthrough. The `gpu` renderer still tone-maps HDR video to SDR.

With the OpenGL backend, hardware decoding works without a copy to RAM. On Intel/Linux it needs EGL and `MPV_RENDER_PARAM_WL_DISPLAY` ([render_gl.h L84-L100](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/render_gl.h#L84-L100)).

The header describes the SW backend as "extremely simple (but slow) … You probably don't want to use this". It does color conversion, scaling and OSD "on the CPU, single-threaded", and "HDR may not work properly" ([render.h L125-L160](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/render.h#L125-L160)). The output format can be `bgr0`, which matches the BGRA of GPUI ([render.h L360-L385](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/render.h#L360-L385)).

Render calls must run on their own thread. That thread must not wait on normal libmpv calls ([render.h L56-L80](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/render.h#L56-L80)).

`libmpv2` 6.0.0 wraps only the OpenGL render API. `RenderParamApiType` has one value, `OpenGl` ([render.rs](https://docs.rs/crate/libmpv2/6.0.0/source/src/mpv/render.rs)). The SW constants exist only in the raw `libmpv2-sys` bindings (`MPV_RENDER_API_TYPE_SW`, `MPV_RENDER_PARAM_SW_SIZE = 17`). So option A needs `unsafe` FFI calls through `libmpv2-sys`.

## The options

### A. SW render API, CPU frames, one GPUI image for each frame

To build it, make a render context with `MPV_RENDER_API_TYPE_SW`. Render each frame to a `bgr0` buffer, wrap the buffer in a `RenderImage`, and paint it with `img()`. Then drop the old frame.

The table gives the cost per frame, at 4 bytes per pixel:

| Size | One frame | 24 fps | 60 fps |
|---|---|---|---|
| 1920×1080 | 8.3 MB | 199 MB/s | 498 MB/s |
| 3840×2160 | 33.2 MB | 796 MB/s | 1.99 GB/s |

Each frame goes through memory at least three times. mpv writes it, the app wraps it, and wgpu uploads it to the atlas. mpv also scales and converts the color on one CPU core (see render.h above). With hardware decoding, mpv must copy each decoded frame back from the GPU to RAM (a `-copy` hwdec mode). That is one more copy.

What the user sees: 1080p will probably play, with high CPU use, more heat and more fan noise. 4K will probably drop frames. HDR looks wrong or flat. Subtitles and the Lua OSD also render on the CPU, so playback gets slower when they show.

[cijiugechu/gpui-video-player](https://github.com/cijiugechu/gpui-video-player/tree/beb23b09f64d670446099ab902cc048074917993) shows that this image path works in GPUI. It uses GStreamer, not mpv, and it uses this path on every platform except macOS. Its [docs/video-memory.md](https://github.com/cijiugechu/gpui-video-player/blob/beb23b09f64d670446099ab902cc048074917993/docs/video-memory.md) records the GPU memory leak when the app does not drop old frames, and the `drop_image` fix.

Option A2 is a variant. It uses the OpenGL render API with an offscreen EGL context, and reads the pixels back with `glReadPixels`. mpv then scales and converts on the GPU, and hardware decoding needs no copy. But the readback and the atlas upload stay. A2 is faster than A, but each frame still goes through RAM.

### B. OpenGL render API into a texture that the GPUI renderer shares

In this option, mpv renders into a GL texture. The app exports the texture as a dmabuf (a Linux handle to GPU memory) with `EGL_MESA_image_dma_buf_export`. Then it imports the dmabuf into the wgpu device of GPUI with the Vulkan extension `VK_EXT_external_memory_dma_buf`, and syncs the two APIs.

This is not possible at the pinned version without a fork of GPUI. GPUI gives no public access to its wgpu device, and the wgpu renderer has no code for `PrimitiveBatch::Surfaces` (see above). The fork must add a Linux `PaintSurface` variant, a texture import, a draw path, and GPU sync. Then the fork must follow every GPUI update. This repo pins `gpui-pre` and `gpui-kit` together, so a fork also breaks that chain.

### C. A Wayland subsurface that the app owns, with mpv drawing into it

A subsurface is a child surface that the compositor draws as part of its parent window. To build it:

1. Get the `wl_surface` and the `wl_display` of the GPUI window from `HasWindowHandle` and `HasDisplayHandle`.
2. Open the same connection with `Backend::from_foreign_display`, and bind `wl_subcompositor`.
3. Make a child surface, a `wl_egl_window` on it, and an EGL surface.
4. Create the mpv OpenGL render context on that EGL context, with `MPV_RENDER_PARAM_WL_DISPLAY`.
5. On a render thread, call `mpv_render_context_render` and `eglSwapBuffers`.

[mpvpaper](https://github.com/GhostNaN/mpvpaper/blob/fc5ed285d657c31fbd063b854884ea00e72d2d51/src/main.c) shows that the mpv part works. It does the same on a layer-shell surface. It creates the context with `MPV_RENDER_PARAM_WL_DISPLAY` and `MPV_RENDER_API_TYPE_OPENGL` (L572-L579), makes a `wl_egl_window` on its own `wl_surface` (L890), and calls `mpv_render_context_render` and `eglSwapBuffers` (L181-L191).

These Wayland protocol facts apply to the subsurface:

- A `set_position` change applies only when the parent surface commits ([wayland.xml L3179-L3198](https://gitlab.freedesktop.org/wayland/wayland/-/blob/1.24.0/protocol/wayland.xml#L3179)). The parent is the GPUI window, so the video moves to a new layout position only on the next GPUI frame.
- In sync mode, a commit of the child waits for a commit of the parent ([wayland.xml L3109-L3150](https://gitlab.freedesktop.org/wayland/wayland/-/blob/1.24.0/protocol/wayland.xml#L3109)). GPUI does not commit when it is idle, so the video subsurface must use `set_desync`.
- `place_above` and `place_below` set the order of the surfaces ([wayland.xml L3220](https://gitlab.freedesktop.org/wayland/wayland/-/blob/1.24.0/protocol/wayland.xml#L3220)). If the video is above, it covers GPUI, and the controls stay in the Lua OSD that mpv draws into the same frame. If the video is below, GPUI must draw a transparent hole where the video is. The GPUI Wayland window supports a background that is not opaque ([wayland/window.rs L642](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_linux/src/linux/wayland/window.rs#L642)). With that order, GPUI can draw its own controls over the video.
- Set an empty input region on the video surface ([wayland.xml L1635](https://gitlab.freedesktop.org/wayland/wayland/-/blob/1.24.0/protocol/wayland.xml#L1635)). Then the pointer and the keys go to the GPUI surface, and GPUI does not lose focus to a surface that it ignores.

What the user sees: no frame goes through RAM, and hardware decoding makes no copy. 4K plays as well as it does in the mpv window today, but HDR passthrough stops (the `gpu` renderer, not `gpu-next`). Keys and clicks no longer go to mpv. The app must send them with the `keypress` and `mouse` commands, or handle them in GPUI.

The new code is Wayland object handling in `unsafe` code, an EGL setup, and a render thread. `wayland-client` and `khronos-egl` are already in the lock. The app also needs `wl_egl_window`, from the `egl` feature of `wayland-sys` or from the `wayland-egl` crate. The subsurface lives outside GPUI, and GPUI does not know about it. If a GPUI update changes how GPUI owns its `wl_surface`, this code can break.

### D. Keep the separate window, and make it feel part of the app

Hyprland window rules can open the player on the same workspace, fullscreen or tiled in place of the library window ([Hyprland wiki: Window Rules](https://wiki.hypr.land/Configuring/Basics/Window-Rules/)). Omarchy already has rules for `mpv` app ids in `~/.local/share/omarchy/default/hypr/apps/system.lua`. This keeps `gpu-next` and HDR, and needs no new code. It does not put the video in the library window. ADR 0002 already notes that on Wayland, a GPUI window cannot sit on top of the player window.

## Track lists for subtitles and audio

The `track-list` property lists all tracks. Each entry has `id` (the value for `--sid` and `--aid`, unique only within one type), `type` (`audio`, `video`, `sub`), `title`, `lang`, `codec`, `default`, `forced`, `external`, `external-filename`, `selected`, and more fields ([input.rst L3308-L3420](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/input.rst#L3308)). [input.rst L3485](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/input.rst#L3485) gives the full node layout, with the type of each field. `current-tracks/audio` and `current-tracks/sub` point to the selected entry ([input.rst L3541](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/input.rst#L3541)).

To select a track, set `sid` or `aid` to an `id`, or to `no` ([options.rst, `--aid` and `--sid`](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst)). The app already does this in `Player::select_subtitle`.

`mpv_observe_property` sends the first value, then one event for each change, merged when changes come fast ([client.h L1173-L1200](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h#L1173)).

Do not observe `track-list` with `Format::Node` in `libmpv2` 6.0.0. The event thread panics. `PropertyData` has no node variant, and `from_raw` calls `unimplemented!()` for a format that it does not know ([events.rs L33-L62](https://docs.rs/crate/libmpv2/6.0.0/source/src/mpv/events.rs#33)). `GetData` is also implemented only for `f64`, `i64`, `bool` and `String`. Observe `track-list` with `Format::String` instead. For a node property, the string form in mpv is JSON, because `print_node` calls `json_write` ([m_option.c L3830-L3838](https://github.com/mpv-player/mpv/blob/v0.41.0/options/m_option.c#L3830-L3838)). The app already depends on `serde_json`. A new selection changes the `selected` field in the JSON, so selection changes also arrive as events.

A smaller first step works with the current mpv window. mpv has a built-in list picker. `select.lua` gives the script bindings `select-sid` and `select-aid` ([select.rst L60-L76](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/select.rst#L60)). It is on by default (`--load-select`, [options.rst L1062](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst#L1062)). The app sets `load-scripts=false`, but that option turns off only the script folder of the user ([options.rst L817](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst#L817)). So the `a` and `s` keys in `silo-controls.lua` can run `script-binding select/select-aid` and `script-binding select/select-sid` in place of `cycle aid` and `cycle sid`. The menu uses the console style of mpv, not the Omarchy theme colors. For a menu in the theme colors, `silo-controls.lua` can call `mp.input.select` with its own items ([lua.rst L1009](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/lua.rst#L1009)), or draw the list with its own OSD code.

## Recommendation

1. Add the track lists now, in the current mpv window. Start with the built-in `select-sid` and `select-aid` bindings, which take a few lines of Lua. If the menu must use the theme colors, draw it in `silo-controls.lua`. If the app needs the list itself, for example for a GPUI menu later, observe `track-list` as a string and parse the JSON.
2. If the user wants video in the app window, build a prototype of option C. It is the only option with no copy for each frame and no GPUI fork. Put the video subsurface above the GPUI surface first, and keep the Lua OSD for the controls. Then ADR 0002 stays valid.
3. Do not use option A for normal playback. It is easy to build, but 4K and HDR suffer. It is acceptable only for a small preview, for example a 480p video in the library.
4. Do not use option B. It needs a GPUI fork for as long as the app exists.
5. Option C replaces the statement in ADR 0001 that "mpv opens its own Wayland window with `gpu-next`". If the app moves to option C, record that in a new ADR.

## Open risks

- Options A, B and C all use the render API, which uses the `gpu` renderer. HDR passthrough and `gpu-next` stop. Only option D keeps them. The user must accept this before work starts.
- Option C depends on two GPUI behaviors that GPUI does not document. GPUI must give a libwayland `wl_surface`, and GPUI must ignore input on unknown surfaces. A `gpui-pre` update can change either one.
- In desync mode, the video shows its frames on its own. During a window resize, the GPUI layout and the video position can disagree for one frame, because `set_position` waits for the next GPUI commit.
- With an empty input region on the video, mpv gets no keys or clicks. The app must send them to mpv with the `keypress` and `mouse` commands. If it does not, the click targets of the Lua controller stop working.
- Today mpv makes its own window fullscreen. In option C, the GPUI window must go fullscreen, and the subsurface must fill it.
- No prototype ran on this machine. The plan for option C and the cost numbers for option A come from the sources and from arithmetic, not from a measurement.
- A crash in the render thread closes the app. ADR 0001 already accepts this risk for libmpv.
