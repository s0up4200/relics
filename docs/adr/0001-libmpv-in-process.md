# libmpv runs inside the app process

Status: superseded in part by [ADR 0003](0003-video-in-the-library-window.md). libmpv still runs in the app process, but mpv no longer opens its own window.

The app plays video with libmpv inside its own process. mpv opens its own Wayland window with `gpu-next`, and the app uses libmpv only to send commands and read properties and events. We also built a version that starts mpv as a child process and controls it over JSON IPC on a socketpair. That version keeps the app alive when mpv crashes, and it can hide the token and stop orphan processes. We chose libmpv because it gives a typed API and avoids the known JSON IPC faults: a number sent to `set` can fail without an error, and teardown can wait forever for a reply. We accept that a crash in mpv closes the whole app.
