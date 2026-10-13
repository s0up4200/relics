---
name: install-local
description: Build silo-omarchy from this checkout and install it for the current user, so that Silo shows in the Omarchy launcher.
disable-model-invocation: true
---

This skill installs the app from source for one user, until the AUR package exists. It needs no `sudo`. An install replaces the copies of an earlier install.

1. Build the release binary. Run the build from the root of this checkout:

   ```sh
   CARGO_TARGET_DIR=$HOME/.cache/silo-omarchy-target nice -n 19 cargo build --release -j 4
   ```

   The step is done when cargo prints `Finished`. If the build fails, stop and report the error.

2. Install the binary and the desktop entry. Before you install, note which of the two target files exist, for the report. `install -D` makes the folders when they do not exist, and replaces a file that exists:

   ```sh
   install -Dm0755 "$HOME/.cache/silo-omarchy-target/release/silo-omarchy" "$HOME/.local/bin/silo-omarchy"
   install -Dm0644 data/silo-omarchy.desktop "$HOME/.local/share/applications/silo-omarchy.desktop"
   ```

3. Make the window opaque. Omarchy makes each window a little transparent, and the video plays inside the Library window. The rule is in `data/silo-omarchy.lua`.
   - If `~/.config/hypr/hyprland.lua` already contains `^silo-omarchy$`, the rule is in place. Go to step 4.
   - Otherwise, copy the file to `~/.config/hypr/hyprland.lua.bak.<unix time>`. Then add the `o.window` call from `data/silo-omarchy.lua` on the line after `require("default.hypr.omarchy")`.
   - Run `hyprctl reload`, then `hyprctl configerrors`. The step is done when `configerrors` prints no error. If it prints an error, put the backup back and report the error.

4. Report to the user: the version from `Cargo.toml`, whether each file was new or replaced, and whether the rule was added or already in place. Tell the user that Silo shows in the launcher (Super + Space), and that a running copy of the app keeps the old version until it restarts.
