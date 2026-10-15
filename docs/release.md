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

Run `/release-tag`. It does these steps:

1. It opens a pull request that raises `version` in `Cargo.toml` and `Cargo.lock`, and waits until you merge it. If `Cargo.toml` already has the new version, it skips this step.
2. On `main`, it creates a signed tag for the version, for example `v0.2.0`, and pushes it.

The Release workflow starts on the tag. It makes sure that the tag is signed and agrees with `Cargo.toml`, and builds the package from `packaging/aur/PKGBUILD` in an Arch Linux container. Then it publishes the GitHub release at once, with the package, `sha256sums.txt`, and notes that git-cliff writes from the PR titles (see `cliff.toml`).

The PR title check makes each PR title start with a type, such as `feat:` or `fix:`. git-cliff uses the type to put the PR under New Features, Bug Fixes or Other Changes. A title with `!` before the colon goes under Breaking Changes.

To test the package build before you tag, start the Release workflow by hand on a branch (`gh workflow run release.yml --ref <branch>`). A manual run does not create a release. Its run summary shows the notes for the commits since the last tag.

## Omarchy updates

The Hyprland rule goes in `/usr/share/omarchy/default/hypr/apps/`. At each major Omarchy release, make sure that `default/hypr/apps.lua` in Omarchy still loads the files from that directory. If the rule does not load, the Library window and the video in it are 1.5% to 4% transparent.
