# silo-omarchy

silo-omarchy is a native media player for a Silo server. The Library window follows the live Omarchy theme. mpv plays the video in a separate Player window. The app also works on other Wayland desktops.

## Install

Install the `silo-omarchy` package from the AUR with your AUR helper:

```sh
yay -S silo-omarchy
```

The package builds the app from source. The build takes some minutes.

## Make the Player window opaque in Omarchy

Omarchy makes each window a little transparent. The package adds a Hyprland rule that makes the Player window opaque. The rule is in `/usr/share/omarchy/default/hypr/apps/silo-omarchy.lua`.

After you install the package, run this command to load the rule:

```sh
hyprctl reload
```

If the Player window stays transparent, add the rule to your own Hyprland configuration:

1. Open `~/.config/hypr/hyprland.lua`.
2. Find the line `require("default.hypr.omarchy")`.
3. Add these lines below it:

   ```lua
   o.window("^silo-player$", {
     tag = "-default-opacity",
     opacity = "1 1",
   })
   ```

4. Run `hyprctl reload`.

## Other desktops

On a Wayland desktop without Omarchy, you install and start the app in the same way. You do not need the Hyprland rule.

## Build from source

You need Rust, `mpv`, `clang` and `cmake`.

```sh
cargo build --release
```

The player controls are in the file `data/silo-controls.lua`. The app looks for this file in these directories, in this order:

1. The directory in the `SILO_OMARCHY_DATA` environment variable.
2. `$XDG_DATA_HOME/silo-omarchy/` (by default `~/.local/share/silo-omarchy/`).
3. `silo-omarchy/` in each directory of `$XDG_DATA_DIRS` (by default `/usr/local/share` and `/usr/share`).

If you run the app from the repository, set `SILO_OMARCHY_DATA`:

```sh
SILO_OMARCHY_DATA=data target/release/silo-omarchy
```

If the app cannot find the file, the video plays without controls.

## Release

The maintainer steps are in [docs/release.md](docs/release.md).
