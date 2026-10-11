# Packaging and Hyprland rules

Research for issue #18. The facts are from 2026-10-11. They cover Omarchy 4.0.4 (released 2026-09-15), Hyprland 0.56.2, mpv master, and the Arch Developer Manual.

## Answers

1. A package adds a window rule when it installs one Lua file in `/usr/share/omarchy/default/hypr/apps/`. Omarchy loads every `*.lua` file in that folder.
2. The AUR package is a source package named `silo-omarchy`. It builds from the tagged release tarball. Add `silo-omarchy-bin` only if the build time hurts users.
3. Install `silo-controls.lua` at `/usr/share/silo-omarchy/silo-controls.lua`. The app finds it through `XDG_DATA_DIRS` and gives it to mpv with the `script` option.
4. Release with a signed `vX.Y.Z` tag. CI builds in an Arch container. The PKGBUILD points at the tag tarball.
5. Update the gpui-kit pin about once a month, or when a feature needs it. Pick the newest versions that are 7 days old or more.

## 1. How a package adds a Hyprland window rule

Facts:

- Omarchy 4.0.4 writes the Hyprland configuration in Lua. The user file `~/.config/hypr/hyprland.lua` runs `dofile(".../default/hypr/bootstrap.lua")` and then `require("default.hypr.omarchy")`. Source: the local install in `/usr/share/omarchy` (package `omarchy-settings` 4.0.4-1).
- `bootstrap.lua` sets `package.path` to `~/.local/state`, `~/.config`, and `$OMARCHY_PATH` (default `/usr/share/omarchy`), in that order. Source: `/usr/share/omarchy/default/hypr/bootstrap.lua`.
- `windows.lua` requires `default.hypr.apps`. `apps.lua` calls `require_all.files(<omarchy_path>/default/hypr/apps, "default.hypr.apps")`. `require_all` runs `find <dir> -maxdepth 1 -type f -name '*.lua' | sort` and requires each file. The same code is in `basecamp/omarchy`, file `default/hypr/apps.lua`.
- The header of `require_all.lua` calls this folder an "extension-style folder". Omarchy ships 19 app files there, for example `webcam-overlay.lua`, `pip.lua`, and `system.lua`.
- `windows.lua` tags every window `default-opacity`. Then it loads the app files. Last, it sets `opacity = "0.985 0.96"` on windows that still have the tag. An app file removes the tag with `tag = "-default-opacity"`.
- `o.window(match, rules)` wraps `hl.window_rule`. A string `match` is a class regex. Source: `default/hypr/helpers.lua`.
- `system.lua` floats every class that matches the regex `(...|imv|mpv)`. The regex is not anchored. It sets opacity 1 for the anchored class `mpv`. The id `silo-player` does not contain `mpv`, so neither rule matches it. The player needs its own opacity rule.
- `webcam-overlay.lua` is the model to copy. It uses a dedicated app id "to keep this out of mpv's generic centered floating rules".
- mpv sets its Wayland app id with `--wayland-app-id=<string>` (default `mpv`). Source: mpv `DOCS/man/options.rst`.
- `~/.config/hypr/*.lua` belongs to the user. A package must not write there.
- Omarchy ships pacman hooks that pause and resume the Hyprland reload when `omarchy-settings` changes. No hook reloads Hyprland for a third-party package.

Recommendation: the `silo-omarchy` package installs `/usr/share/omarchy/default/hypr/apps/silo-omarchy.lua`:

```lua
-- silo-omarchy: the player window is video, so it gets no transparency.
o.window("^silo-player$", {
  tag = "-default-opacity",
  opacity = "1 1",
})
```

- Add no float rule. The map says the player opens fullscreen through mpv (#9). Add no rule for `silo-omarchy`, the library window. It tiles and takes the default opacity.
- Hyprland reads a new file at the next reload or login. A root install script cannot reload the session of a user. The README tells the user to run `hyprctl reload`.
- Fallback for a system without this folder: put the same lines in `~/.config/hypr/hyprland.lua` after the `require("default.hypr.omarchy")` line. The README documents this for Hyprland users who do not run Omarchy.
- Risk: the folder belongs to the `omarchy` packages. pacman accepts files from another package in that folder. Omarchy can rename the folder in a later release. The app does not need the rule to run. Without it, the player is 1.5 to 4 percent transparent. Make sure that `default/hypr/apps.lua` still exists at each Omarchy major release.
- A system without Omarchy gets one unused file in a folder that Omarchy would own. This is harmless. A split package is not worth the work for the first version.

## 2. AUR package shape

Facts:

- AUR packages are `PKGBUILD` files. The user builds them with `makepkg`. Source: Arch Wiki, "Arch User Repository".
- The Arch Rust guideline names the package after the binary, in lowercase. It fetches crates in `prepare()` with `cargo fetch --locked --target host-tuple`. It builds with `cargo build --frozen --release --all-features` and `CARGO_TARGET_DIR=target`. It tests with `cargo test --frozen`. It installs with `install -Dm0755 -t "$pkgdir/usr/bin/" "target/release/$pkgname"`. Source: manual.archlinux.page, "Rust package guidelines".
- The guideline says nothing about `-bin` or `-git`. The common AUR convention is this: the plain name builds a release tarball, `-git` builds the head of the repository, and `-bin` repackages a prebuilt binary. `zed-bin` is an example. It downloads the upstream release tarball and sets `provides` and `conflicts` for the plain name (AUR mirror `archlinux/aur`, branch `zed-bin`).
- The AUR web site blocks scripted fetches (Anubis). The mirror at `github.com/archlinux/aur` has one branch for each package.
- I did not measure the build time or the system dependency list of a GPUI app. Measure both when the first PKGBUILD exists.

Recommendation:

- Name the package `silo-omarchy`. Build from the GitHub tag tarball. A user can read the source, the checksum is fixed, and no binary hosting is needed.
- `depends`: `mpv` (libmpv), `wayland`, `libxkbcommon`, `vulkan-icd-loader`, `fontconfig`, and whatever `ldd` shows. `makedepends`: `cargo`. Fill the exact list from a clean Arch container build (#7 sets up that CI).
- Commit `Cargo.lock`. The `--locked` fetch needs it.
- `package()` installs the binary to `/usr/bin/silo-omarchy`, `silo-controls.lua` to `/usr/share/silo-omarchy/`, the Hyprland rule from section 1, a `.desktop` file, and an icon.
- Add `silo-omarchy-bin` later if users report long builds. CI would then attach a binary tarball to each release. Skip it now, because it doubles the release work.
- Make no `-git` package.

## 3. Where `silo-controls.lua` lives and how the app finds it

Facts:

- mpv loads scripts from the `scripts` folder of its config directory. It also loads the files given to `--script` or `--scripts=a.lua:b.lua`. Source: mpv `DOCS/man/lua.rst` and `options.rst`.
- libmpv loads no config files by default. It loads them only when the app sets `config=yes` and `config-dir`. Source: the `mpv_create` comment in mpv `include/mpv/client.h`. So the auto-load folder does not work for the app, and an explicit `script` option does.
- Arch packages ship mpv Lua scripts in two ways. `mpv-uosc` (AUR) installs to `/usr/share/mpv/scripts/uosc/`. `mpv-modernz` (AUR) and `mpv-mpris` (extra) install to `/etc/mpv/scripts/`. Both depend on the mpv command line tool, which reads those folders. Sources: the AUR mirror PKGBUILDs and `pacman -Ql mpv-mpris`. The plain `mpv` tool would also load our script from those folders. This is wrong for a script that belongs to one app.

Recommendation:

- Install the script at `/usr/share/silo-omarchy/silo-controls.lua`. It is data of one app, so it stays out of the shared mpv folders.
- The app searches in this order and uses the first match: the env variable `SILO_OMARCHY_DATA` (for `cargo run` and tests), then `$XDG_DATA_HOME/silo-omarchy/`, then each entry of `XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`) plus `/silo-omarchy/`.
- The app sets the mpv option `script` to the absolute path before `mpv_initialize`. If the app finds no file, it logs a warning and plays with no on-screen controls. So it still works on a plain Wayland system with no package files.
- Skip `include_str!` embedding for now. The first package is on the AUR and installs the file. Embedding is a small change if `cargo install` matters later.

## 4. Release process

Facts:

- The Arch guideline wants a fixed `Cargo.lock`, an offline build, and a checksum in the PKGBUILD. Source: Rust package guidelines.
- A GitHub tag has an automatic tarball at `https://github.com/<owner>/<repo>/archive/refs/tags/vX.Y.Z.tar.gz`. `mpv-uosc` uses this form.
- The plan for CI is an Arch container (#7).

Recommendation:

1. Raise the version in `Cargo.toml`, update `Cargo.lock`, and merge to `main`.
2. Create a signed tag `vX.Y.Z` with the `release-tag` skill, and push it.
3. CI on the tag builds and tests in the Arch container, then creates a GitHub release.
4. In a separate AUR clone, raise `pkgver`, run `updpkgsums`, run `makepkg -f` in a clean chroot, regenerate `.SRCINFO`, and push to the AUR.
5. Keep the PKGBUILD in `packaging/aur/` in this repository as a template, so reviewers see each change. Copy it to the AUR by hand. Automate the AUR push only if releases become frequent.

## 5. gpui-kit pin cadence

Facts (crates.io API and GitHub, 2026-10-11):

- Zed's gpui crates are republished as `gpui-pre`. The workflow `release-gpui.yml` in `longbridge/gpui-kit` runs on the cron `0 0 * * 1`, every Monday at 00:00 UTC. It publishes only when Zed changed the published crates.
- `gpui-pre` dates: 0.3.5 on 09-14, 0.3.6 on 09-21, 0.3.7 on 09-28, 0.3.8 on 10-05. That is one release each week.
- `gpui-kit` dates: 0.6.0 on 09-03, 0.6.1 on 09-09, 0.6.2 and 0.6.4 on 09-18, 0.6.6 on 09-21, 0.7.0 on 09-28, 0.7.1 on 10-05. That is 8 releases in 5 weeks. The authors yanked 0.6.5.
- `gpui-omarchy` dates: 0.1.3 on 09-25, 0.2.0 on 09-28, 0.2.1 on 10-05.
- gpui-kit pins one gpui-pre snapshot exactly, because a snapshot can change its API (the research report on the architecture cites `Cargo.toml#L63-L72`).
- Under the 7-day age rule, 0.7.1, 0.3.8 and 0.2.1 (all from 2026-10-05) are usable from 2026-10-12.

Recommendation:

- Pin exact versions with `=` in `Cargo.toml` for `gpui-kit`, `gpui-pre*` and `gpui-omarchy`. Commit `Cargo.lock`.
- Update monthly, on the first Monday, and also when a needed fix ships. Take the newest set that is 7 days old or more and that resolves to one `gpui-pre` version. Do not follow the weekly snapshots. Each snapshot can break the API, and each bump costs a full rebuild.
- One bump is one PR. It changes the three pins together and runs the full CI.
- Before a release tag, make sure that no pinned version is yanked.

## Needs the user

Nothing. Section 1 depends on an Omarchy folder that can change. The recommendation handles that with a fallback and a re-check at each Omarchy major release.

## Sources

- Local, read-only: `/usr/share/omarchy/default/hypr/{bootstrap,paths,omarchy,windows,apps,require_all,require_optional,helpers}.lua`, `apps/{system,webcam-overlay,pip}.lua`, `~/.config/hypr/hyprland.lua`, `/usr/share/libalpm/hooks/*omarchy*`.
- https://github.com/basecamp/omarchy (`default/hypr/`, releases)
- https://manual.archlinux.page/package-guidelines/rust/
- https://wiki.archlinux.org/title/Arch_User_Repository
- https://github.com/archlinux/aur (branches `mpv-uosc`, `mpv-modernz`, `zed-bin`)
- https://github.com/mpv-player/mpv (`DOCS/man/lua.rst`, `options.rst`, `include/mpv/client.h`)
- https://crates.io/api/v1/crates/{gpui-kit,gpui-pre,gpui-omarchy}
- https://github.com/longbridge/gpui-kit (`.github/workflows/release-gpui.yml`, README)
