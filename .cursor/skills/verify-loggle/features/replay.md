# Replay a fixture

Replay a fixture lets a user open Loggle on `fixtures/mixed-service-investigation.log`, keep the TUI up after `cat` exits, and see twelve parsed events with source and level columns.

## Sub-features

- `replay-open` starts the TUI on the mixed-service fixture with a chosen page id.
- `replay-parse` shows Compose-style and bracket-prefixed sources in the log list.
- `replay-stay` keeps the viewer open after the `cat` command finishes.
- `replay-identity` shows `loggle follow` and `id=verify-<run>` in the header.

## How to get to it (user POV)

- From this repository, run `loggle --id <id> -- cat fixtures/mixed-service-investigation.log`.
- From this repository, run `cargo run -- --id fixture -- cat fixtures/mixed-service-investigation.log`.
- For verification, run `.cursor/skills/verify-loggle/scripts/verify-loggle launch`.

## Driving it with verify-loggle

Preconditions:

- `tmux` is on `PATH`.
- `target/debug/loggle` exists or cargo can build it.
- No other drive owns this `VERIFY_LOGGLE_RUN`.

- **Launch.** Start the isolated fixture session. Run `.cursor/skills/verify-loggle/scripts/verify-loggle launch`. The command prints `ok` with a `page_id` and `session`.
- **Doctor.** Check the instance. Run `.cursor/skills/verify-loggle/scripts/verify-loggle doctor`. Exit code `0`, `loggle pages` lists `verify-<run>`, and the pane contains `retained 12` and `id=verify-<run>`.
- **Read sources.** Capture the list. Run `.cursor/skills/verify-loggle/scripts/verify-loggle capture replay/pane.txt`. The capture contains `minio-init`, `api`, `worker`, and `database` rows, including `job insert rejected` and `request completed requestId=fixture-success statusCode=201`.
- **Proof.** The same capture shows `loggle follow`, `visible 12`, and `markers 0`. The TUI is still running; `doctor` still passes.

## Gotchas

- A `cargo test` pass is not this feature. The proof is the live pane.
- Launch execs `target/debug/loggle` as the tmux pane command. Do not wrap it in a login shell. That wrapper breaks when the default shell is nu.
- Pipe mode (`cat fixture | loggle`) is a different entry point. Do not report it verified because command mode worked.
- `dc` and `docker compose up` are out of this recipe. The fixture is the default verification instance.
- Pane width is 200x40 so the header `id=` and footer filters stay readable. A narrower pane truncates filter values to `~`.
