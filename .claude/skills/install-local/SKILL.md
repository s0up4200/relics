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

   Then update the desktop database. The browser reads it to find the app for the `org.siloserver.silo:` return link of Browser sign-in:

   ```sh
   update-desktop-database "$HOME/.local/share/applications"
   ```

3. Report to the user: the version from `Cargo.toml`, and whether each file was new or replaced. Tell the user that Silo shows in the launcher (Super + Space), and that a running copy of the app keeps the old version until it restarts.
