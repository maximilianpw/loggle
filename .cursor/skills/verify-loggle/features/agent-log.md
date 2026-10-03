# Agent log access

Agent log access lets a user list live Loggle pages and print recent raw records from another terminal without taking over the TUI.

## Sub-features

- `pages-list` lists the active page id, pid, age, and command.
- `log-tail` prints the last N matching records from that page.
- `log-property` returns whole records for an exact property filter.
- `log-source-text` combines `--service` and `--text` on the same page.

## How to get to it (user POV)

- With a page still open, run `loggle pages`.
- Run `loggle log -i <id> -n 5`.
- Run `loggle log -i <id> -n 5 --property requestId=fixture-failed`.
- Run `loggle log -i <id> -n 10 --service database --property requestId=fixture-failed`.

## Driving it with verify-loggle

Preconditions:

- The fixture session from [replay.md](./replay.md) is healthy.
- `eval "$(.cursor/skills/verify-loggle/scripts/verify-loggle env)"` has been run.

- **List pages.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle cli -- pages`. Exit code `0`. Stdout starts with `ID	PID	AGE	COMMAND` and contains `$PAGE_ID` plus `cat` and `mixed-service-investigation.log`.
- **Failed request.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle cli -- log -i "$PAGE_ID" -n 100 --property requestId=fixture-failed`. Exit code `0`. Stdout is the five failed-request records, including the seven-line API error block with `cause: "job insert rejected: missing synthetic parent"`. It does not contain `fixture-failed-extra` or `fixture-success`.
- **One record.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle cli -- log -i "$PAGE_ID" -n 1 --property requestId=fixture-failed`. Stdout is only the last matching record, the seven-line API error block.
- **Database slice.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle cli -- log -i "$PAGE_ID" -n 10 --service database --property requestId=fixture-failed`. Stdout is the one `job insert rejected` JSON line.
- **Success request.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle cli -- log -i "$PAGE_ID" -n 10 --property requestId=fixture-success`. Stdout is the five success records ending in `statusCode=201`.
- **Proof.** Write those command outputs under `artifacts/<run>/agent-log/`. The TUI pane still shows `id=$PAGE_ID`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle capture agent-log/pane.txt`.

## Gotchas

- `loggle pages` without this run's `XDG_STATE_HOME` lists the user's pages, or none. Always go through `verify-loggle cli`.
- Property filters are exact. `requestId=fixture` matches nothing. `requestId=fixture-failed` does not match `fixture-failed-extra`.
- `-n` counts matching records, not raw lines. The failed API error is one record and seven lines.
- The page log is removed when the session is reaped. Query it before `cleanup`.
