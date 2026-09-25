---
name: verify-loggle
description: >-
  Drive the Loggle TUI and loggle pages/log CLI in an isolated tmux session
  and capture pane plus page-log evidence. Use when proving Loggle behavior,
  verifying a TUI, search, details, filter, or page-log change, or when a
  feature map recipe in this skill must be exercised live.
---

# Verify Loggle

Loggle is a terminal log viewer. The primary surface is the TUI. The secondary surface is `loggle pages` and `loggle log` against a live page. Drive both through `.cursor/skills/verify-loggle/scripts/verify-loggle`. Do not attach to a Loggle the user already has open.

Read [features/README.md](features/README.md) before a drive. Use the matching feature file as the recipe.

## Launch

Build once, then start a disposable TUI that replays `fixtures/mixed-service-investigation.log`. Ready means the helper prints `ok` and `doctor` sees `retained 12` plus `id=<page-id>`.

```sh
.cursor/skills/verify-loggle/scripts/verify-loggle build
.cursor/skills/verify-loggle/scripts/verify-loggle launch
eval "$(.cursor/skills/verify-loggle/scripts/verify-loggle env)"
```

Launch creates a tmux session `loggle-verify-<run>` at 200x40, sets `XDG_STATE_HOME` under `$TMPDIR/loggle-verify/<run>/state` (`/tmp` when `TMPDIR` is unset), and runs:

```text
target/debug/loggle --no-color --id verify-<run> -- cat fixtures/mixed-service-investigation.log
```

A second `launch` with a healthy last run reuses it. For a fresh pane, `cleanup` then `launch`. Teardown is `cleanup`. It kills that tmux session and removes that run's state dir. It leaves evidence in `.cursor/skills/verify-loggle/artifacts/<run>/`.

## Doctor

Run this first whenever the pane looks wrong, after a failed drive, and before the first drive of a run.

```sh
.cursor/skills/verify-loggle/scripts/verify-loggle doctor
```

Pass means all of these hold:

- `target/debug/loggle` exists
- tmux still has this run's session
- `loggle pages` with this run's `XDG_STATE_HOME` lists `verify-<run>`
- the pane contains ` loggle `, `id=verify-<run>`, and `retained 12`

Fail means stop driving. `cleanup` then `launch`. Do not type into another tmux session or the user's default `~/.local/state/loggle`.

## Drive

Load the run, then send keys and read the pane. Stable handles are header text, footer prompts, dialog titles, and `loggle` CLI flags. Do not aim at row indexes unless the recipe names them.

```sh
eval "$(.cursor/skills/verify-loggle/scripts/verify-loggle env)"
.cursor/skills/verify-loggle/scripts/verify-loggle keys -- / job-001 Enter
.cursor/skills/verify-loggle/scripts/verify-loggle capture search/pane.txt
.cursor/skills/verify-loggle/scripts/verify-loggle cli -- log -i "$PAGE_ID" -n 5 --property requestId=fixture-failed
```

TUI keys from `src/commands.rs` and the README Controls section:

| User action | Keys |
|---|---|
| Search | `/` then text then `Enter` |
| Source filter | `s` then source then `Enter` |
| Level filter | `l` then level then `Enter` |
| Show property filter | `+` then `key=value` then `Enter` |
| Details | `Enter` |
| Command palette | `?` |
| Clear filters | `c` |
| Jump top | `g` `g` |
| Quit | `q` |

Prompt labels in the footer: `/`, `source: `, `level: `, `show prop: `, `hide prop: `. Dialog titles: `Commands`, `Property filters`, `Pinned fields`, `Filter presets`, `Sources`. Header tokens: `loggle follow` or `loggle paused`, `retained`, `visible`, `id=`. Details pane tokens: `details source=`, `message `, `> <key> =`.

`keys --` arguments are passed to `tmux send-keys`. Named keys such as `Enter`, `Escape`, `Space`, and `C-d` stay named.

After `cat` finishes, the TUI stays open until `q`. The page log stays readable through `cli -- log` until cleanup.

## Evidence

Put proof under `.cursor/skills/verify-loggle/artifacts/<run>/`. Cleanup must not delete that directory.

A proof includes the action and the resulting state:

- TUI: pane capture before or at the action, then after. The after capture must show the header identity (`loggle`, `id=verify-<run>`) and the named end state from the feature file.
- CLI: the exact `cli --` command, stdout, stderr, and exit code.
- Page-log mutation or filter: a second `loggle log` read of the stored lines, not only the TUI.

Drive the real binary through tmux and `loggle pages`/`log`. Do not call `App` methods from a test as a substitute for a mapped user path. Unit tests in `src/` are not this skill's proof.

Default launch uses the repo fixture. It does not prove Docker Compose, `loggle start`, or a live service.

## Cleanup

```sh
.cursor/skills/verify-loggle/scripts/verify-loggle cleanup
```

That command kills the tmux session recorded in the run env and deletes `$TMPDIR/loggle-verify/<run>/`. It does not kill processes by name. It does not delete `artifacts/`. After cleanup, confirm the capture files still exist at the paths `capture` printed.

Two verification runs can coexist if each has its own `VERIFY_LOGGLE_RUN`. Do not share `XDG_STATE_HOME` with the user or with another run.

## Helpers

`.cursor/skills/verify-loggle/scripts/verify-loggle` is executable. Invoke it with a path from the repo root, as in the commands above.

`env` prints `export` lines for `PAGE_ID`, `SESSION`, `STATE_HOME`, `BIN`, and `EVIDENCE_DIR`. `capture` with a relative path writes under that run's evidence directory.
