# The player uses a custom Lua controller, and the app owns the skip logic

Status: superseded by [ADR 0003](0003-video-in-the-library-window.md). The app no longer uses the Lua controller. The app still owns the skip logic.

The player window uses `silo-controls.lua`, which the app loads into libmpv with the `scripts` option. The built-in mpv controller is off. The script draws the seek bar, the marker ranges, the track names and the skip prompt in the colors of the live Omarchy theme. The app sends the theme, the markers and the prompt to the script with `script-message`, and the script sends `silo-action` back. The skip state machine stays in the app, so the tests can reach it without mpv.

We also built a version with the built-in mpv controller. It needs no Lua, but its prompt is OSD text that the user cannot click, and it cannot use the theme colors. A separate GPUI overlay window does not work on Wayland, because an app cannot place its window on top of the player window.
