# Relics

Relics is an unofficial Silo client for Linux, with extras for Omarchy. The library window uses GPUI and gpui-omarchy. libmpv plays video in its own window.

## Builds

A full GPUI build takes every core it can get and about 16 GB of disk per target dir, and other agents share the machine.

- Run every cargo command as `CARGO_TARGET_DIR=$HOME/.cache/relics-target taskset -c 0-3 nice -n 19 cargo ... -j 4`. The shared target dir also applies in a worktree, so a new worktree rebuilds only the app crate.
- Run one cargo command at a time, and one implementer agent at a time.
- Clean up before your final reply of the task:
  1. Stop each rig that you started, as `docs/rig.md` says.
  2. Delete `target/` in the worktree, if a cargo command ran without `CARGO_TARGET_DIR`.
  3. If `du -sh ~/.cache/relics-target` shows more than 30 GB and `pgrep -x cargo` finds no build, delete `~/.cache/relics-target`.

## Live runs

To run the real app against a fake Silo server, change the Server, and take screenshots, follow `docs/rig.md`.

## Pull requests

To show screenshots or recordings in a pull request: if brrbin is on `PATH`, run `brrbin publish --help` and follow it. If it exits with code 4, stop and ask the user to run `brrbin login`. If brrbin is not installed, or it exits with code 6, attach the screenshots to the pull request. Mask each token in every capture. A brrbin Evidence is private to the maintainers, so publish it without a review by the user. A screenshot that you attach to the pull request is public, because the repo is public. Also mask the Silo server address in it, and show it to the user before you attach it.

## Agent skills

### Issue tracker

Issues live in GitHub Issues on s0up4200/relics. Use the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The repo uses the six default triage labels. Each label has the same name as its role. See `docs/agents/triage-labels.md`.

### Domain docs

The repo has a single context, with `GLOSSARY.md` and `docs/adr/` at the root. See `docs/agents/domain.md`.
