<p align="center">
  <img src="data/relics.svg" alt="Relics icon" width="128">
</p>

<h1 align="center">Relics</h1>

<p align="center">Unofficial Silo client for Linux. Follows your Omarchy theme.</p>

Relics is a native media player for a Silo server. The Library window follows the live Omarchy theme. mpv plays the video inside the Library window. The app also works on other Wayland desktops.

Not affiliated with the Silo project.

## Install

Relics is an Arch Linux package for x86_64. Each [release](https://github.com/s0up4200/relics/releases/latest) has the package and a checksum file.

1. Download `relics-<version>-1-x86_64.pkg.tar.zst` and `sha256sums.txt` from the latest release into one folder.
2. In that folder, make sure that the download is complete:

   ```sh
   sha256sum -c sha256sums.txt
   ```

3. Install the package:

   ```sh
   sudo pacman -U relics-*.pkg.tar.zst
   ```

To update, do the same steps with the new release. To get a notice for each new release, click **Watch** > **Custom** > **Releases** on the GitHub page of the repository.

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

Run `relics --version` to show the version and source commit without opening a window.

If you use Claude Code in this checkout, run `/install-local`. It builds the app and installs it for your user in `~/.local`, so that Relics shows in the Omarchy launcher. This copy takes priority over the package. To go back to the package, remove `~/.local/bin/relics` and `~/.local/share/applications/relics.desktop`.
Settings and the startup log show the same build identity. `-dirty` means that tracked files had local changes at build time.
If the source has no commit metadata, the build shows `unknown` and emits a build warning. Release packages require a known commit.

## Release

The maintainer steps are in [docs/release.md](docs/release.md).
