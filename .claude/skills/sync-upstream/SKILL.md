---
name: sync-upstream
description: Pull new upstream code into Octodock and its patched GPUI fork. Use when asked to sync, update, or merge upstream for octodock, zeron, zui, or gpui.
---

# Sync upstream

Two repositories move together. Sync both in one pass, in the order below.

- Octodock: repo root, branch `my-custom`, `origin` = `octopuslowtech/octodock`, `upstream` = `zeronsh/zeron`.
- GPUI fork: `vendor/zui` (git-ignored clone), branch `ui-zoom`, `origin` = `octopuslowtech/zui-fork`, `upstream` = `zeronsh/zui`.

`ui-zoom` is the zui rev that upstream Zeron pins plus our commits (`Window::set_zoom`).
The root `Cargo.toml` pins the `ui-zoom` head in five places: `gpui`, `gpui_platform`, `gpui_tokio` under `[workspace.dependencies]`, and `gpui`, `gpui_macros` under `[patch."https://github.com/zeronsh/zui"]`. The patch section exists because `gpui-base` still depends on `zeronsh/zui`; without it two copies of gpui get linked.

## 1. Preconditions

- `git status --short` in the repo root must be empty. If not, stop and ask the user to commit or stash first. Never stash or discard for them.
- If `vendor/zui` is missing: `git clone git@github-octopuslowtech:octopuslowtech/zui-fork.git vendor/zui`, then `git -C vendor/zui remote add upstream https://github.com/zeronsh/zui` and `git -C vendor/zui checkout ui-zoom`.
- `git -C vendor/zui status --short` must be empty and the branch must be `ui-zoom`.

## 2. Fetch and report

```
git fetch upstream
git -C vendor/zui fetch upstream
git -C vendor/zui fetch origin
git rev-list --count my-custom..upstream/main
git show upstream/main:Cargo.toml | grep -m1 'zeronsh/zui'
```

The last command shows the zui rev upstream now pins; call it `ZUI_REV`. Compare it with `git -C vendor/zui merge-base ui-zoom upstream/main`. Tell the user how many Zeron commits are incoming and whether the zui rev moved. If both are zero and unchanged, stop: nothing to sync.

## 3. Merge Zeron into Octodock

```
git branch backup/my-custom-before-sync-$(date +%Y%m%d)
git merge upstream/main
```

Resolve conflicts by keeping our customizations and taking everything else from upstream:

- To learn what our side intended in a file: `git log --oneline upstream/main..my-custom -- <file>`.
- Files we deleted on purpose (iOS app, deploy/TestFlight/installer workflows, translated READMEs) stay deleted.
- `Cargo.toml` gpui lines: take upstream's features and comments, keep the `octopuslowtech/zui-fork` URL and the `[patch]` section. The rev is set in step 5.
- `Cargo.lock`: take upstream's version; it is regenerated in step 6.
- A conflict you cannot attribute to a deliberate customization: stop and ask.

Finish with `git merge --continue`. Do not push.

## 4. Bring the GPUI fork to the new rev

Skip this step if `ZUI_REV` is already an ancestor of `ui-zoom`.

```
git -C vendor/zui merge <ZUI_REV>
git -C vendor/zui push origin ui-zoom
git -C vendor/zui push origin upstream/main:main
```

Merge, never rebase: older Octodock commits pin older `ui-zoom` revs, and a force-push would orphan them.
After resolving conflicts, confirm the patch survived: `grep -n 'pub fn set_zoom' vendor/zui/crates/gpui/src/window.rs` must match, and every `platform_window.scale_factor()`, `content_size()`, and `mouse_position()` read in `Window::bounds_changed` must still apply `self.zoom`.

## 5. Repin

Set all five revs in the root `Cargo.toml` to `git -C vendor/zui rev-parse ui-zoom`, and update the base rev named in the comment above them. `.github/workflows/macos.yml` reads the URL and rev from `Cargo.toml`, so it needs no edit.

## 6. Verify

AGENTS.md requires approval before builds and tests. Ask once, naming these commands, then run them:

```
cargo check -p zeron-ui --all-targets
cargo test --locked -p zeron-ui --lib
```

The first runs without `--locked` so `Cargo.lock` picks up the new rev. Afterwards `grep -c 'zeronsh/zui' Cargo.lock` must print `0`; any other number means a crate still resolves to the unpatched source and needs a `[patch]` entry.

Fetching the fork requires the git CLI because this machine rewrites `https://github.com/octopuslowtech/` to an SSH alias; `.cargo/config.toml` sets `git-fetch-with-cli` for that. Keep it.

## 7. Report

Commit the repin and lockfile only if the user asked for commits. State: Zeron commits merged, old and new zui rev, conflicts resolved and how, commands run with their results, and that nothing was pushed to `octopuslowtech/octodock`.
