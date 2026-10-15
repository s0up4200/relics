# Research: build and publish releases in GitHub Actions

Date: 2026-10-15. Status: research only. No code changed.

This research answers one question. When the maintainer pushes a signed tag with the `/release-tag` skill, how does CI build Relics and publish a GitHub release with a changelog and files to download?

## Short answer

Keep the Release workflow that the repo already has. It starts on a `v*` tag, checks the tag, and builds the package in an Arch Linux container. Do not replace it with dist, GoReleaser, or release-plz. Make these four changes:

1. Attach the built package (`.pkg.tar.zst`) and a SHA-256 checksum file to the release. Today the workflow builds the package and then discards it.
2. Write the release notes with git-cliff. Group the commits by their conventional-commit type, with the same headings that GoReleaser uses in mkbrr and qui.
3. Turn off the `debug` option of makepkg in the container, so that the build fits on the disk of the runner.
4. Raise `version` in `Cargo.toml` in a pull request before you run `/release-tag`. The workflow refuses a tag that does not agree with `Cargo.toml`, and the skill does not change the file.

## What the repo has now

### The Release workflow

[`.github/workflows/release.yml`](../../.github/workflows/release.yml) already does most of the work:

- It starts on `push: tags: ['v*']` and on `workflow_dispatch`.
- It runs in the `archlinux:latest` container on `ubuntu-latest`.
- It makes sure that the tag is `v` plus the `version` in `Cargo.toml`. It asks the GitHub API whether the tag is an annotated tag and whether GitHub can verify its signature (`.verification.verified`).
- It makes a source tarball with `git archive`, sets `pkgver` in [`packaging/aur/PKGBUILD`](../../packaging/aur/PKGBUILD), and runs `makepkg --syncdeps` as a normal user.
- On a tag, it runs `gh release create "$GITHUB_REF_NAME" --verify-tag --generate-notes`.

The workflow did not run yet. `gh run list --workflow release.yml` returns no runs, and the repo has no tags and no releases (checked 2026-10-15).

The current workflow has three problems.

The release has no files. `gh release create` gets no file names, so the package that makepkg built is lost when the job ends.

The notes have no groups. `--generate-notes` uses the GitHub release-notes API. The notes list the merged pull requests, the contributors, and a link to the full changelog. The API can group the pull requests only by label ([GitHub docs, automatically generated release notes](https://docs.github.com/en/repositories/releasing-projects-on-github/automatically-generated-release-notes)). Relics does not label its pull requests in a consistent way. Of the last 15 merged pull requests, 7 have the label `bug` and 8 have no label. #98 `feat: show which Relics build runs` has the label `bug` (`gh pr list --state merged`). The pull request titles do follow conventional commits.

The build can fill the disk. Arch's default `/etc/makepkg.conf` has `OPTIONS=(... debug lto)`, and `/etc/makepkg.conf.d/rust.conf` sets `DEBUG_RUSTFLAGS="-C debuginfo=2"` (read from `pacman 7.1.0` on this machine). With `debug`, makepkg adds those flags. So the release build gets full debug info, and makepkg also makes a second `relics-debug` package. Then the `check()` function of the PKGBUILD runs `cargo test`, which builds all crates a second time in the test profile, also with full debug info. The CI workflow already says that full debug info "makes the target dir larger than the free disk of the runner, and the linker can crash" ([`ci.yml`](../../.github/workflows/ci.yml)). A standard Linux runner in a public repo has 4 CPUs, 16 GB of RAM, and a 14 GB SSD ([GitHub docs, GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)). `sudo -u builder` resets the environment, so a `CARGO_PROFILE_*` variable on the job does not get to makepkg.

### The CI workflow

[`ci.yml`](../../.github/workflows/ci.yml) runs clippy and the tests in the same Arch container. It installs `base-devel clang cmake git rust mpv wayland libxkbcommon libxkbcommon-x11 libxcb fontconfig freetype2 alsa-lib vulkan-headers`. It uses `Swatinem/rust-cache@v2.9.2`. The last run on `main` took 3 minutes 24 seconds with a warm cache (run [38701396955](https://github.com/s0up4200/relics/actions/runs/38701396955)).

### Why an Arch container and not Ubuntu packages

The comment in `ci.yml` says "Arch Linux, because the Ubuntu runners have an old mpv". The Ubuntu archive has these `libmpv-dev` versions: 0.37.0 in 24.04 (noble, which is `ubuntu-latest`), 0.40.0 in 25.10, and 0.41.0 in 26.04 ([packages.ubuntu.com](https://packages.ubuntu.com/search?keywords=libmpv-dev&searchon=names&exact=1)). Arch has mpv 0.41.0 ([archlinux.org package search](https://archlinux.org/packages/search/json/?name=mpv)). The `libmpv2` crate itself needs only libmpv 2.0, which is mpv 0.35.0 ([docs.rs, libmpv2 6.0.0](https://docs.rs/crate/libmpv2/6.0.0)).

The users of Relics run Omarchy, which is Arch. A binary that CI builds on Arch links against the same libmpv and glibc that those users have. So the Arch container is the correct build host for the release, and the release needs no Ubuntu package list.

For reference, Zed's own dependency script installs this list on Arch: `gcc clang lld musl cmake alsa-lib fontconfig glib2 libva wayland libgit2 libxcb libxkbcommon-x11 llvm openssl zstd pkgconf sqlite pipewire xdg-desktop-portal jq git` ([zed `script/linux`](https://raw.githubusercontent.com/zed-industries/zed/main/script/linux)). On Ubuntu it installs `libasound2-dev libfontconfig-dev libwayland-dev libx11-xcb-dev libxkbcommon-x11-dev libvulkan1 cmake clang` and more. Relics needs fewer of these, because GPUI as a library does not use libgit2, sqlite, or musl. The `makedepends` and `depends` in the PKGBUILD, with `makepkg --syncdeps`, already install what the build needs. If an entry is missing, the build fails.

### How the build identity gets its version

`relics --version`, Settings, and the startup log show `BUILD_IDENTITY` ([`src/lib.rs`](../../src/lib.rs) line 36):

```rust
concat!("Relics ", env!("CARGO_PKG_VERSION"), " (", env!("RELICS_REVISION"), ")")
```

The version comes from `version` in `Cargo.toml`. Nothing else sets it. So the tagged commit must have the correct version in `Cargo.toml`. CI cannot set the version from the tag, because the AUR package builds from the GitHub tarball of the tag, and that tarball has the `Cargo.toml` of the tagged commit.

The commit comes from [`build.rs`](../../build.rs). In a git checkout, it runs `git rev-parse HEAD`. In a tarball, it reads `.git-revision`. `.gitattributes` marks that file `export-subst`, so `git archive` and the GitHub tarball write the commit into it. The Release workflow uses `git archive`, so the package gets the commit. The `check()` function of the PKGBUILD fails when the identity ends in `(unknown)`.

### The `/release-tag` skill

[`~/.claude/skills/release-tag/SKILL.md`](file:///home/soup/.claude/skills/release-tag/SKILL.md) does these steps:

1. It pulls the release branch (`main` here).
2. It finds the latest tag. If the repo has no tags, it starts from `v0.1.0`.
3. It picks a minor bump for `feat` commits, and a patch bump in other cases. It never bumps major.
4. It asks the user to confirm.
5. It runs `git tag -s v<VERSION> -m "v<VERSION>"` and `git push origin v<VERSION>`.
6. It shows the tag and the releases page.
7. Only when a workflow opens a release-notes pull request (it greps `.github/workflows/` for `docs(...): add $`), it waits for that pull request. If grep finds nothing, "the release is finished after step 6".

Relics fits this flow with one gap:

- The tag is signed and annotated (`-s`, `-m`). The signature check of the workflow agrees with this.
- Step 7 does not apply. Relics has no docs site and no changelog file, so it needs no release-notes pull request. The changelog goes in the body of the GitHub release. mkbrr needs the pull request because its changelog is in `documentation/changelog.mdx` ([mkbrr `changelog.yml`](file:///home/soup/github/autobrr/mkbrr/.github/workflows/changelog.yml)).
- The skill does not change `Cargo.toml`. If the skill tags `v0.2.0` while `Cargo.toml` says `0.1.0`, the workflow fails at "Check the version and the tag signature". [`docs/release.md`](../release.md) already says to raise `version` and merge it to `main` first. `main` needs a pull request (the ruleset "main: pull requests"), so the bump is a short pull request such as `chore: release v0.2.0`. Then run `/release-tag` and pick the same version.
- Only admins can create `v*` tags (the ruleset "Release tags: admins only", from `gh api repos/s0up4200/relics/rulesets`). So a bot with `GITHUB_TOKEN` cannot make the tag. This rules out tools that make the tag themselves (see release-plz below).

### The changelog style of the sibling projects

mkbrr and qui use GoReleaser with `changelog: use: github`, sorted oldest first. They drop merge commits and group the commit subjects by regular expression ([mkbrr `.goreleaser.yml`](file:///home/soup/github/autobrr/mkbrr/.goreleaser.yml), [qui `.goreleaser.yml`](file:///home/soup/github/autobrr/qui/.goreleaser.yml) lines 104-126):

| Order | mkbrr heading | qui heading | Match |
|---|---|---|---|
| 0 | Breaking Changes | Breaking Changes | `^\w+(\([^)]*\))?!:` |
| 10 | New Features | New Features | `feat` |
| 20 | Bug fixes | Bug Fixes | `fix` |
| 30 or 999 | Other work | Other Changes | all other commits |

qui writes each line as `{{ .Message }} (@{{ .Author.Username }})`. mkbrr adds the footer `**Full Changelog**: https://github.com/autobrr/mkbrr/compare/{{ .PreviousTag }}...{{ .Tag }}`. Use the same groups for Relics, so that its releases read like the others.

## The options

| Tool | Builds a GUI app with system libraries | Package formats | Changelog | Fits the signed `/release-tag` flow | Status (from `gh api`, 2026-10-15) |
|---|---|---|---|---|---|
| Current workflow: Arch container, makepkg, `gh release create` | Yes. `ci.yml` already builds the app this way | Arch package (`.pkg.tar.zst`). The PKGBUILD is also the AUR template | GitHub generated notes, grouped by label only | Yes | Already in the repo |
| [dist (cargo-dist)](https://axodotdev.github.io/cargo-dist/book/) | Possible. `dependencies` installs apt packages before the build, and `github-build-setup` adds steps ([config](https://axodotdev.github.io/cargo-dist/book/reference/config.html)) | Archives, shell and PowerShell installers, npm, Homebrew, MSI. No `.deb`, AppImage, or Arch package | From a `CHANGELOG` file (`package.changelog`) | Yes, it starts on a tag. It generates its own `release.yml` and expects to own it | Active. v0.33.0, 2026-09-11 |
| [GoReleaser, Rust builder](https://goreleaser.com/customization/builds/rust/) | Weak. It builds with `cargo zigbuild` by default, or with `cross`. The docs do not cover links to a system libmpv or Wayland through zig. You install Rust, Zig, and cargo-zigbuild yourself | Archives. nfpm makes `.deb`, `.rpm`, `.apk`, and Arch packages (mkbrr uses all four) | `use: github` with regex groups, the same as mkbrr and qui | Yes | Active. v2.18.3, 2026-10-09 |
| [release-plz](https://release-plz.dev/docs/usage/release) | Does not build binaries | None | Writes `CHANGELOG.md` with git-cliff in a release pull request | No. It creates the tag itself (`<package>-v<version>`, or `v<version>` for one package) and runs `cargo publish`. The ruleset blocks bot tags, and `Cargo.toml` has `publish = false` | Active. v0.3.172, 2026-10-12 |
| [git-cliff](https://git-cliff.org/docs/usage/args) | Not a build tool | None | Yes. `--latest` "processes the commits starting from the latest tag". `--current` "processes the commits that belong to the current tag". `--strip` removes the header and footer. With `GITHUB_TOKEN`, it adds PR numbers and usernames ([GitHub integration](https://git-cliff.org/docs/integration/github)) | Yes, it only reads history | Active. v2.14.2, 2026-09-18. In Arch `extra` as `git-cliff` 2.14.2 |
| [softprops/action-gh-release](https://github.com/softprops/action-gh-release) | Not a build tool | Uploads any files | Generated notes or a body file | Yes | Active. v3.0.3, 2026-08-30 |
| [taiki-e/upload-rust-binary-action](https://raw.githubusercontent.com/taiki-e/upload-rust-binary-action/main/README.md) | Builds with cargo, cross, or cargo-zigbuild. The README does not cover system libraries | `.tar.gz` or `.zip` of the binary, plus `include` files | None. It needs a release that `create-gh-release-action` made | Yes | Active. v1.30.2, 2026-04-17 |
| [`gh release create`](https://cli.github.com/manual/gh_release_create) | Not a build tool | Uploads the files that you give as arguments. `file#label` sets a display name | `--generate-notes`, or `--notes-file` | Yes. `--verify-tag` stops if the tag is not on the remote | `github-cli` 2.102.0 in Arch `extra` |

What the table shows:

- dist and GoReleaser cross-compile a portable CLI binary for many systems. Relics is one Linux GUI app with one host (Arch, x86_64), and it already has a working build in a container. Both tools also expect to own the workflow file. Of the two, only GoReleaser (through nfpm) makes an Arch package, and its nfpm config would copy the `depends` list of the PKGBUILD.
- release-plz does not fit, because it makes the tag itself.
- `gh` is already in the container. `softprops/action-gh-release` adds nothing that `gh release create` cannot do here.
- git-cliff gives the grouped changelog of the sibling projects with one configuration file. It is one `pacman` package, with no new action.

## Recommendation

Keep `release.yml` and change it as follows. This sketch is not tested.

```yaml
name: Release

# A signed tag vX.Y.Z builds the Arch package and creates a GitHub release with the package and the changelog.
# A manual run (workflow_dispatch) builds the package for the current version and creates no release.
on:
  push:
    tags: ['v*']
  workflow_dispatch:

permissions:
  contents: write

jobs:
  release:
    runs-on: ubuntu-latest
    container: archlinux:latest
    timeout-minutes: 60
    env:
      GH_TOKEN: ${{ github.token }}
    steps:
      - name: Install packages
        run: pacman -Syu --noconfirm --needed base-devel git github-cli git-cliff
      - uses: actions/checkout@v5
        with:
          # git-cliff needs all commits and tags to find the previous release.
          fetch-depth: 0
      - name: Check the version and the tag signature
        id: version
        run: |
          # (unchanged)
      - name: Build the package
        env:
          VERSION: ${{ steps.version.outputs.version }}
        run: |
          git config --global --add safe.directory "$GITHUB_WORKSPACE"
          # Full debug info makes the target dir larger than the free disk of the runner.
          sed -i 's/^OPTIONS=(\(.*\) debug /OPTIONS=(\1 !debug /' /etc/makepkg.conf
          git archive --prefix="relics-$VERSION/" -o "packaging/aur/relics-$VERSION.tar.gz" HEAD
          sed -i "s/^pkgver=.*/pkgver=$VERSION/" packaging/aur/PKGBUILD
          useradd -m builder
          echo 'builder ALL=(ALL) NOPASSWD: /usr/bin/pacman' > /etc/sudoers.d/builder
          chown -R builder packaging/aur
          cd packaging/aur
          sudo -u builder makepkg --syncdeps --noconfirm
          sha256sum relics-*.pkg.tar.zst > sha256sums.txt
      - name: Create the GitHub release
        if: github.ref_type == 'tag'
        run: |
          git cliff --current --strip all --github-repo "$GITHUB_REPOSITORY" -o notes.md
          gh release create "$GITHUB_REF_NAME" --repo "$GITHUB_REPOSITORY" --verify-tag \
            --title "$GITHUB_REF_NAME" --notes-file notes.md \
            packaging/aur/relics-*.pkg.tar.zst packaging/aur/sha256sums.txt
```

Notes on the sketch:

- Artifact names. makepkg names the file `relics-<pkgver>-<pkgrel>-x86_64.pkg.tar.zst` (`PKGEXT='.pkg.tar.zst'` in `/etc/makepkg.conf`). Keep that name, because Arch users know it. A user installs it with `sudo pacman -U relics-0.2.0-1-x86_64.pkg.tar.zst`.
- The release comes last. The workflow creates the release only after the build and `check()` pass. So a failed build leaves no empty release.
- git-cliff configuration. Add `cliff.toml` at the repo root. Start from the `github` template of `git cliff --init github` ([GitHub integration](https://git-cliff.org/docs/integration/github)), so that each line shows the PR number and `@username`. Use `commit_parsers` to put `!:` in "Breaking Changes", `feat` in "New Features", `fix` in "Bug Fixes", and all other commits in "Other Changes", in that order. Add the "Full Changelog" compare link as the footer, as mkbrr does.
- Commits without a type. Relics squash-merges its pull requests, so each pull request is one commit with the pull request title as its subject. PR #99 `Support normal text editing and Ctrl/Super shortcuts` has no type prefix, so it goes in "Other Changes".
- First release. The repo has no tags. For the first tag, `--current` takes all commits since the start, which is a long list. You can write the notes for the first release by hand with `gh release edit`.
- Cache. The workflow does not need `Swatinem/rust-cache`. A tag run can restore caches from the default branch ([GitHub docs, dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)). But the cache from `ci.yml` holds dev-profile builds with other flags, so a release build gets little from it. GitHub deletes caches that are not used for 7 days, and releases are rare. Accept a cold build and `timeout-minutes: 60`.
- The dry run stays. `gh workflow run release.yml --ref <branch>` builds the package and creates no release, as [`docs/release.md`](../release.md) says. Run it once before the first tag, to measure the build time and the disk use. If you want to test the package file before you tag, add an `actions/upload-artifact` step for manual runs.

### How it fits `/release-tag`

1. Merge a pull request that raises `version` in `Cargo.toml` and `Cargo.lock`.
2. Do the smoke test in [`docs/release.md`](../release.md).
3. Run `/release-tag`. Pick the same version as `Cargo.toml`.
4. The skill pushes the signed tag. The Release workflow checks the tag, builds the package, and creates the release with the notes and the package.
5. The skill finds no release-notes workflow, so it stops after its step 6. Watch the Release run in the Actions tab.

When you make the change, also update step 3 of `docs/release.md`. It must say that the release has the package and the checksum file.

## Open decisions

- Version bump. Keep the manual bump pull request (recommended, small). The other choice is to make `build.rs` read the version from `git describe`, and from an `export-subst` file such as `$Format:%(describe:tags)$` in a tarball. That removes the bump. But `CARGO_PKG_VERSION` is also in the user agent and in the `app_version` that the app sends to the server ([`src/main.rs`](../../src/main.rs) line 86, [`src/api/mod.rs`](../../src/api/mod.rs) line 1324), so all three places must change.
- Package formats. The recommendation ships only the Arch package. A plain `.tar.gz` of the binary works only on a system with the same libmpv soname and a glibc as new as Arch's, so it adds little for Omarchy users. A `.deb` needs an Ubuntu 26.04 build (mpv 0.41) and its own dependency list. An AppImage must bundle mpv and its libraries. Add these only if users on other systems ask.
- aarch64. The PKGBUILD has `arch=('x86_64')`. `ubuntu-24.04-arm` runners are free for public repos ([GitHub docs](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)). The `archlinux` Docker image is x86_64 only, so an arm64 build needs an Arch Linux ARM image or an Ubuntu host. Skip it until a user asks.
- AUR. The AUR has no `relics` package yet (AUR RPC, 2026-10-15). The steps in `docs/release.md` publish it by hand. Later, a `relics-bin` AUR package can install the `.pkg.tar.zst` contents from the release. If CI pushes to the AUR, it needs an SSH key secret for the AUR account.
- PR title check. A `pr-title.yml` workflow (as in mkbrr and qui) makes every commit fit a changelog group.
