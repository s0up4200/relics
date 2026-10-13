# Relics

Relics is an unofficial Silo client for Linux, with extras for Omarchy. The library window uses GPUI and gpui-omarchy. libmpv plays video in its own window.

## Builds

A full GPUI build overloads this machine (16 cores, 19 GB of RAM, about 16 GB per target dir).

- Run every cargo command as `nice -n 19 cargo ... -j 4`, with `CARGO_TARGET_DIR=$HOME/.cache/relics-target`. The shared target dir also applies in a worktree, so a new worktree rebuilds only the app crate.
- Run one cargo command at a time, and one implementer agent at a time.

## Agent skills

### Issue tracker

Issues live in GitHub Issues on s0up4200/relics. Use the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The repo uses the six default triage labels. Each label has the same name as its role. See `docs/agents/triage-labels.md`.

### Domain docs

The repo has a single context, with `GLOSSARY.md` and `docs/adr/` at the root. See `docs/agents/domain.md`.
