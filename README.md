# silo-omarchy

silo-omarchy is a native media player for a Silo server. The Library window follows the live Omarchy theme. mpv plays the video inside the Library window. The app also works on other Wayland desktops.

## Install

Install the `silo-omarchy` package from the AUR with your AUR helper:

```sh
yay -S silo-omarchy
```

The package builds the app from source. The build takes some minutes.

## Make the window opaque in Omarchy

Omarchy makes each window a little transparent. The video plays inside the Library window, so the package adds a Hyprland rule that makes this window opaque. The rule is in `/usr/share/omarchy/default/hypr/apps/silo-omarchy.lua`.

After you install the package, run this command to load the rule:

```sh
hyprctl reload
```

If the window stays transparent, add the rule to your own Hyprland configuration:

1. Open `~/.config/hypr/hyprland.lua`.
2. Find the line `require("default.hypr.omarchy")`.
3. Add these lines below it:

   ```lua
   o.window("^silo-omarchy$", {
     tag = "-default-opacity",
     opacity = "1 1",
   })
   ```

4. Run `hyprctl reload`.

On a Wayland desktop without Omarchy, you do not need the rule.

## Build from source

You need Rust, `mpv`, `clang` and `cmake`.

```sh
cargo build --release
```

The app needs a Wayland desktop to show video.

## Release

The maintainer steps are in [docs/release.md](docs/release.md).
