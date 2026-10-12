# silo-omarchy

silo-omarchy is a native media player for a Silo server. The Library window follows the live Omarchy theme. mpv plays the video inside the Library window. The app also works on other Wayland desktops.

## Install

Install the `silo-omarchy` package from the AUR with your AUR helper:

```sh
yay -S silo-omarchy
```

The package builds the app from source. The build takes some minutes.

## Build from source

You need Rust, `mpv`, `clang` and `cmake`.

```sh
cargo build --release
```

The app needs a Wayland desktop to show video.

## Release

The maintainer steps are in [docs/release.md](docs/release.md).
