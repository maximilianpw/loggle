# Loggle verification map

This directory is the maintained source for verifying the user-facing behavior of Loggle. Read the index before driving the app, then use the matching feature file as the recipe.

## Baseline preconditions

- Build `target/debug/loggle` with `.cursor/skills/verify-loggle/scripts/verify-loggle build`.
- Launch the isolated fixture session with `.cursor/skills/verify-loggle/scripts/verify-loggle launch`.
- Run `.cursor/skills/verify-loggle/scripts/verify-loggle doctor` and require `retained 12` and `id=verify-<run>`.
- `eval "$(.cursor/skills/verify-loggle/scripts/verify-loggle env)"` before `cli --` or `$PAGE_ID`.
- Never drive a Loggle tmux session this run did not start.
- Do not set `XDG_STATE_HOME` to the user's `~/.local/state`.

## Driving conventions

- Start every recipe from the baseline fixture session unless its preconditions say otherwise.
- Prefer header, footer, and dialog title strings over pane coordinates.
- Treat every command as literal. Keep quoted names and flags unchanged.
- Send TUI keys through `verify-loggle keys --`.
- Run `pages` and `log` through `verify-loggle cli --`.
- Restore filters with `c` after a mutating TUI recipe. Do not remove proof artifacts during cleanup.

## Proof and skip reporting

- Capture the user action and the resulting state, not only the final pane.
- TUI proof includes a pane capture that shows `loggle` and `id=verify-<run>`.
- CLI proof includes the command, stdout, stderr, and exit code.
- Page-log proof includes a second `loggle log` read of the stored lines.
- Record the feature ID and entry point used with every artifact.
- Report an unreachable path with the attempted command and the unmet precondition.
- Do not report a skipped entry point as verified through a different path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior. It then uses exactly four H2 sections in this order.

1. `Sub-features` lists short IDs with one line for each behavior.
2. `How to get to it (user POV)` lists every user entry point.
3. `Driving it with verify-loggle` starts with `Preconditions:` and uses labeled bullets that pair each user action with an exact command and observable result.
4. `Gotchas` lists traps that can waste or invalidate a verification run.

Keep implementation details out of the map. Name only user paths, stable handles, required state, commands, and observable proof.

## Features

- [Replay a fixture](./replay.md) covers launching the TUI on the mixed-service fixture and reading retained events.
- [Agent log access](./agent-log.md) covers `loggle pages` and `loggle log` against the live page.
- [Search](./search.md) covers `/` search, match navigation, and clear.
- [Details](./details.md) covers opening the details pane and reading parsed properties.
- [Filters](./filters.md) covers source, level, and property filters.
