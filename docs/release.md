# Release

## Smoke test on the real server

Do this test before each release. Use a test account on the real Silo server and a build of the release commit.

1. Sign in with the test account.
2. Make sure that Home shows its rows with artwork.
3. Play one Item that the server sends as direct play. Make sure that the video and the sound start.
4. Play one Item that the server must transcode. Make sure that the video and the sound start.
5. Stop playback in the middle of the Item. In the Silo web UI, make sure that the Item shows the same progress.
6. Start playback again. Suspend the computer during playback, and then resume it. Make sure that playback continues, or that the Player view shows an error that you can clear with Enter.

If a step fails, do not release.

## Make the release

1. Raise `version` in `Cargo.toml`. Run `cargo check` to update `Cargo.lock`. Merge the change to `main`.
2. On `main`, create a signed tag for the version, for example `git tag -s v0.2.0`, and push it.
3. The Release workflow starts on the tag. It makes sure that the tag is signed and agrees with `Cargo.toml`, builds the package from `packaging/aur/PKGBUILD` in an Arch Linux container, and creates the GitHub release.

To test the package build before you tag, start the Release workflow by hand on a branch (`gh workflow run release.yml --ref <branch>`). A manual run does not create a release.

## Update the AUR package

The repository must be public, because the AUR package downloads the tarball of the tag from GitHub.

1. Copy `packaging/aur/PKGBUILD` to your clone of the AUR repository `silo-omarchy`.
2. Set `pkgver` to the new version and `pkgrel` to `1`.
3. Run `updpkgsums` to replace `SKIP` with the checksum of the tarball.
4. Build the package in a clean chroot (`extra-x86_64-build` from `devtools`).
5. Run `makepkg --printsrcinfo > .SRCINFO`.
6. Commit and push to the AUR.

## Omarchy updates

The Hyprland rule goes in `/usr/share/omarchy/default/hypr/apps/`. At each major Omarchy release, make sure that `default/hypr/apps.lua` in Omarchy still loads the files from that directory. If the rule does not load, the Library window and the video in it are 1.5% to 4% transparent.
