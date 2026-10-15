# Relics

Relics is an unofficial Silo client for Linux, with extras for Omarchy. The library window uses GPUI and gpui-omarchy. libmpv plays video in its own window.

## Builds

A full GPUI build overloads this machine (16 cores, 19 GB of RAM, about 16 GB per target dir).

- Run every cargo command as `nice -n 19 cargo ... -j 4`, with `CARGO_TARGET_DIR=$HOME/.cache/relics-target`. The shared target dir also applies in a worktree, so a new worktree rebuilds only the app crate.
- Run one cargo command at a time, and one implementer agent at a time.

## Pull requests

To show screenshots or recordings in a pull request: if brrbin is on `PATH`, run `brrbin publish --help` and follow it. If it exits with code 4, stop and ask the user to run `brrbin login`. If brrbin is not installed, or it exits with code 6, attach the screenshots to the pull request. The repo is public, so mask the Silo server address and each token in every capture, and show the captures to the user before you attach them.

## Agent skills

### Issue tracker

Issues live in GitHub Issues on s0up4200/relics. Use the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The repo uses the six default triage labels. Each label has the same name as its role. See `docs/agents/triage-labels.md`.

### Domain docs

The repo has a single context, with `GLOSSARY.md` and `docs/adr/` at the root. See `docs/agents/domain.md`.
