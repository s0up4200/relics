<p align="center">
  <img src="data/relics.svg" alt="Relics icon" width="128">
</p>

<h1 align="center">Relics</h1>

<p align="center">Unofficial Silo client for Linux. Follows your Omarchy theme.</p>

Relics is a native media player for a Silo server. The Library window follows the live Omarchy theme. mpv plays the video inside the Library window. The app also works on other Wayland desktops.

Not affiliated with the Silo project.

## Install

Install the `relics` package from the AUR with your AUR helper:

```sh
yay -S relics
```

The package builds the app from source. The build takes some minutes.

## Make the window opaque in Omarchy

Omarchy makes each window a little transparent. The video plays inside the Library window, so the package adds a Hyprland rule that makes this window opaque. The rule is in `/usr/share/omarchy/default/hypr/apps/relics.lua`.

After you install the package, run this command to load the rule:

```sh
hyprctl reload
```

If the window stays transparent, add the rule to your own Hyprland configuration:

1. Open `~/.config/hypr/hyprland.lua`.
2. Find the line `require("default.hypr.omarchy")`.
3. Add these lines below it:

   ```lua
   o.window("^relics$", {
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
