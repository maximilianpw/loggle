# Loggle

Loggle is a terminal log viewer for local, newline-delimited logs. It is built for
Docker Compose and multi-process development workflows such as:

```sh
loggle -- docker compose up
```

It opens a live-tail TUI, parses source-prefixed log lines, infers log levels,
keeps a bounded in-memory buffer, and provides Vim-style navigation and simple
filtering.

```text
Live Docker logs -> source/level parsing -> searchable TUI -> inspect/filter properties
```

## At a Glance

Loggle turns noisy multi-process output into a scannable live log view:

![Loggle running against Docker Compose logs](public/docker.jpg)

![Loggle interactive demo](public/demo.gif)

```text
 loggle follow  retained 1248  visible 42
>    147 api             info    http.request GET /api/v1/inventory 200 96ms requestId=716d1e62 durationMs=96
     148 worker          warn    retrying inventory sync tenantId=tenant-1
     149 frontend        info    VITE ready in 312 ms

 details source=api level=info time=14:06:58.892
 message http.request GET /api/v1/inventory 200 96ms
> messageKey = http.request
  requestId = 716d1e62-46a1-46c0-9099-e939a2e4fbb0
  statusCode = 200
  durationMs = 96

 filters source=-  level=-  search=-  props=-   q quit  / search  Enter details  ? commands
```

- Live tail with pause/resume, scrollback, search, and jump-to-match navigation
- Source, level, text, and structured property filters for narrowing dense logs
- Details pane for inspecting parsed timestamps, levels, messages, and properties
- Display field columns for keeping selected properties such as `requestId` or
  `durationMs` aligned across matching rows
- Command palette and searchable managers for discovering and pruning active
  filters/fields
- Graceful shutdown for launched commands: quit behaves like interrupting the
  foreground process, then escalates if it does not exit

## Why Loggle

Plain `tail -f` is fast, but it leaves you reading one raw stream. `docker
compose logs -f` keeps service names, but it is still hard to pause, inspect,
filter, and jump around once output gets noisy. Loggle keeps the local terminal
workflow and adds the pieces you usually reach for in heavier log tools:

| Need | `tail -f` | `docker compose logs -f` | Loggle |
|---|---:|---:|---:|
| Live stream | yes | yes | yes |
| Pause and scroll without losing context | no | no | yes |
| Source and level columns | no | partial | yes |
| Text/source/level filters | no | limited | yes |
| Structured property inspection | no | no | yes |
| Property filters and inline fields | no | no | yes |
| Command-owned shutdown | no | no | yes |

## Installation

Install from crates.io:

```sh
cargo install loggle
```

Install the prebuilt binary with Homebrew:

```sh
brew install maximilianpw/tap/loggle
```

Install the latest GitHub Release binary with the generated installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/maximilianpw/loggle/releases/latest/download/loggle-installer.sh | sh
```

## Usage

From this repository:

```sh
cargo run -- -- docker compose up
```

Try it without Docker:

```sh
cargo run -- sh -c 'i=0; while true; do i=$((i+1)); echo "api | INFO request completed"; echo "[14:06:58.892] INFO (#$i):"; echo "{ requestId: \"demo-$i\", statusCode: 200, durationMs: $((20 + i)) }"; sleep 1; done'
```

Or install it locally:

```sh
cargo install --path .
```

Then run it from any Compose project:

```sh
loggle dc
loggle dcb
loggle -- docker compose up
loggle pages
loggle log -i 1 -n 5
loggle log -i 1 -n 5 --service api
loggle log -i 1 -n 5 --text database
loggle log -i 1 -n 5 --property requestId=716d1e62
loggle --id api -- docker compose up
loggle start
loggle start libre
```

`loggle dc` is an exact shortcut for `loggle -- docker compose up`, and
`loggle dcb` is an exact shortcut for `loggle -- docker compose build`. Only
bare `dc` and `dcb` are special; use the `-- docker compose ...` form for other
Compose commands.

The `--` form is recommended because Loggle starts the command itself and
captures both stdout and stderr. That prevents Docker Compose or service output
from writing directly over the TUI.

Pipe mode also works:

```sh
docker compose up 2>&1 | loggle
```

When using pipe mode, include `2>&1`; otherwise stderr can bypass Loggle.

### Agent Log Access

Every running Loggle page gets an ID automatically:

```sh
loggle -- docker compose up
```

The ID is shown in the top-right corner of the Loggle header while the page is
running. You can also list active pages from another terminal:

```sh
loggle pages
```

Example output:

```text
ID	PID	AGE	COMMAND
1	48291	3m	docker compose up
```

Another terminal or AI agent can then fetch recent raw lines from that page:

```sh
loggle log -i 1 -n 5
```

Use `--id` when you want to choose a stable human-readable ID yourself:

```sh
loggle --id api -- docker compose up
```

Filter the tail to a service/source, text query, or parsed property:

```sh
loggle log -i 1 -n 5 --service api
loggle log -i 1 -n 5 --text "database unavailable"
loggle log -i 1 -n 5 --source worker --property tenantId=tenant-1
loggle log -i 1 -n 5 --service api --text error --property tenantId=tenant-1
loggle log -i 1 -n 5 --property requestId
```

Text filters match the same event fields as TUI search: raw line, parsed
message, source, and property keys/values. Property filters use the same syntax
as the TUI property prompt: `key`, `key=value`, `key!=value`, and `!key`. A
filtered tail returns whole matching records — the header line plus any folded
multi-line property block — and `-n` counts matching records rather than
individual lines.

Page logs are stored in Loggle's local state directory and flushed as input is
drained, so the read command can inspect a live session without taking over the
TUI. Each log retains roughly the same window as the in-memory buffer
(`--buffer-lines`), and a page's log is removed once its session ends and is
reaped. The page log is best-effort: if it cannot be written, the viewer keeps
running and shows a notice instead of exiting. Pass `--no-page-log` to opt out
of writing logs to disk entirely.

### Start Configs

Use `loggle start` to launch a project-local `.loggle.toml` from the current
directory:

```toml
root = "/Users/max-vev/Local/librestock"
source_fields = ["service", "app", "logger"]

[commands]
api = ["pnpm", "--filter", "api", "dev"]
web = ["pnpm", "--filter", "web", "dev"]
```

`root` and `[commands]` are required. Each command is an argv array, runs from
`root`, and is displayed with its table key as the source prefix, such as
`[api]`.

Commands can also use an advanced table form when startup ordering matters:

```toml
root = ".."
env = { NODE_ENV = "development" }

[commands.db]
argv = ["docker", "compose", "-f", "meta/docker-compose.yml", "up", "postgres"]

[commands.db.ready]
command = [
  "docker",
  "compose",
  "-f",
  "meta/docker-compose.yml",
  "exec",
  "-T",
  "postgres",
  "pg_isready",
  "-U",
  "postgres",
]
ms = 500
timeout_ms = 30000

[commands.api]
argv = ["pnpm", "--filter", "@librestock/api", "start"]
wait_for = ["db"]
env = { DATABASE_URL = "postgres://postgres:postgres@localhost:5432/librestock" }

[commands.web]
argv = ["pnpm", "--filter", "@librestock/web", "dev"]
wait_for = ["api"]
```

`wait_for` delays a command until each named dependency is ready. A dependency
without a `ready` block is ready immediately after it starts. Readiness supports
one strategy per command:

- `ready.line = "text"`: ready when stdout or stderr contains the substring.
- `ready.command = ["cmd", "args"]`: ready when the probe exits successfully.

`ready.timeout_ms` defaults to `30000`. `ready.ms` sets the probe interval for
`ready.command` and defaults to `500`. Successful probe output is not shown in
Loggle; timeout errors include recent probe output when there is any.

Top-level `env` applies to every `loggle start` command. Per-command `env`
applies only to that command and overrides top-level keys. Loggle still inherits
the environment from the parent shell; config env adds or overrides variables for
the spawned command and any `ready.command` probes. Env values are literal TOML
strings: Loggle does not load `.env` files or expand shell variables.

Use `loggle start <name>` for reusable named configs. Named configs live at
`$XDG_CONFIG_HOME/loggle/<name>.toml`, or `~/.config/loggle/<name>.toml` when
`XDG_CONFIG_HOME` is not set.

Config `source_fields` extend source promotion for that session. CLI
`--source-field` values take precedence and are checked before config fields.

## Options

```sh
loggle --buffer-lines 50000 -- docker compose up
loggle --no-color -- docker compose logs -f
loggle --record session.log -- docker compose up
loggle pages
loggle log -i 1 -n 5
loggle log -i 1 -n 5 --service api --property tenantId=tenant-1
loggle --id api -- docker compose up
loggle --source-field service,app < app.log
loggle run --name api -- pnpm start --name web -- pnpm dev
loggle start
loggle start libre
```

- `--buffer-lines <N>`: maximum retained lines, default `100000`
- `--no-color`: disables Loggle's source and severity coloring
- `--record <PATH>`: writes every raw incoming line to a session log file
- `--id <ID>` / `-i <ID>`: uses this page ID instead of an auto-generated ID
- `--no-page-log`: disables the per-session page log used by `loggle log`/`pages`
- `pages`: lists active Loggle pages with ID, PID, age, and command
- `--source-field <FIELD>`: promotes matching parsed properties to the source
  column when no explicit prefix exists. Repeat it or pass comma-separated
  fields, e.g. `--source-field service,app`
- `log -i <ID> -n <N>`: prints the last `N` raw lines from a tagged Loggle page
- `log --source <SOURCE>` / `log --service <SERVICE>`: limits page output to a
  parsed source/service
- `log --property <FILTER>` / `log -p <FILTER>`: limits page output by parsed
  properties. Repeat for multiple required predicates
- `dc`: shortcut for `docker compose up`
- `dcb`: shortcut for `docker compose build`
- `[COMMAND]...`: optional command to run under Loggle after `--`
- `run --name <NAME> -- <COMMAND...>`: launches one or more named commands,
  prefixes each output line with `[NAME]`, and shows them in one Loggle session
- `start [NAME]`: launches commands from `.loggle.toml` in the current
  directory, or from a named config in the Loggle user config directory

## Performance Harness

Run synthetic ingestion, filtering, viewport iteration, and draw timings with:

```sh
cargo run --release --features perf-harness --bin loggle-bench -- --lines 100000 --filter text
```

`--filter` accepts `none`, `text`, `source`, `level`, or `property`.

Add `--json` to emit machine-readable results with timings in microseconds:

```sh
cargo run --release --features perf-harness --bin loggle-bench -- --lines 100000 --filter property --json
```

## Controls

Press `?` to open the in-app command palette:

![Loggle command palette and help screen](public/help.jpg)

### Navigation

- `j` / `k`: move one line down/up
- `Ctrl-d` / `Ctrl-u`: half-page down/up
- `gg`: jump to top
- `G`: jump to bottom and resume following
- `n` / `N`: next/previous search match
- `Space` or `p`: pause/resume following
- `y`: copy the selected raw log line to the clipboard
- `v`: start visual-line selection; move with `j` / `k`, arrows, `Ctrl-d` / `Ctrl-u`, `gg`, or `G`; `y` copies the selected lines and `Esc` cancels

### Filtering

- `/`: set text filter/search
- `s`: set source/service filter
- `l`: set level filter
- `+`: add a show property filter
- `-`: add a hide property filter
- `c`: clear filters
- `u`: undo the previous filter change
- `S`: save the current filters as an in-session preset
- `V`: open searchable saved filter presets
- `e`: export the current visible rows to `loggle-export.log`
- `T`: mark or unmark the selected row
- `O`: open observed source status counts

### Details and Properties

- `Enter`: toggle selected log details
- `[` / `]`: move through properties in the details pane
- `f`: show only rows with the selected property value
- `m`: show the selected property key as a displayed log-row column

### Dialogs

- `M`: open searchable display field picker
- `P`: open searchable property filter manager
- `?`: open/close the command palette
- Display field picker: type to search observed property keys; `j` / `k`, arrows, `Ctrl-d` / `Ctrl-u` move selection; `Enter` toggles a field; `Backspace` removes when search is empty; `Delete` removes a shown field; `Esc` closes
- Property filter manager: type to search; `j` / `k`, arrows, `Ctrl-d` / `Ctrl-u` move selection; `Enter` edits; `Backspace` or `Delete` removes when search is empty; `Esc` closes
- Command palette: `j` / `k`, arrows, `Ctrl-d` / `Ctrl-u` move selection; `Enter` runs; `Esc` closes

### Process Control

- `Esc`: close prompt or clear transient mode
- `q`: quit

When Loggle started a child command, quitting sends the child process group a
terminal-style interrupt first, shows a closing overlay, and escalates only if
the command does not exit. Press `q` again while closing to escalate
immediately.

## Log Parsing

Loggle treats common local-development output as structured events:

| Input shape | Parsed source | Parsed level | Displayed message |
|---|---|---|---|
| `api | INFO started` | `api` | `info` | `started` |
| `[worker] WARN retrying` | `worker` | `warn` | `retrying` |
| `14:06:58.892 INFO request ok` | `unknown` | `info` | `request ok` |
| `INFO request ok` | `unknown` | `info` | `request ok` |
| `INFO ready service=api` | `api` | `info` | `ready service=api` |
| `plain output` | `unknown` | inferred or `unknown` | `plain output` |
| `    at handler` after `[api] ERROR failed` | `api` | inferred or `unknown` | `    at handler` |

The `source | message` form matches Docker Compose output. The `[source]
message` form matches concurrently-style named output, including padded names
such as `[backend ] message` and colored prefixes.

If no supported prefix is found, Loggle looks for parsed properties in this
order: user-provided `--source-field` values, then `source`, `service`, `app`,
`logger`, `target`, and `component`. If none are present, the source is shown as
`unknown`. Explicit prefixes always win over promoted fields. Loggle does not
infer source names from arbitrary standalone message words because that identity
is lost once an upstream tool merges streams without a marker.
Unprefixed continuation lines can inherit the previous explicit source when they
look like part of the same event, such as indented stack frames, `Caused by:`
lines, or structured object fragments. A standalone unprefixed line resets that
source context.

The original raw line is preserved in memory, while the displayed message is
cleaned for terminal use:

- ANSI color/control sequences are stripped
- remaining control characters are removed
- repeated whitespace is compacted for display

Loggle also recognizes structured summary lines where the first fields are a
timestamp and level, or just a level:

```text
14:06:58.892 INFO http.request GET /api/v1/inventory 200 96ms
INFO http.request GET /api/v1/inventory 200 96ms
```

For these rows, the timestamp and level are parsed into structured fields and
the remaining text is shown as the message. Level inference for other logs is
keyword-based and case-insensitive. It recognizes common tokens such as
`fatal`, `error`, `warn`, `info`, `log`, `debug`, `trace`, and `verbose`.

Structured property blocks printed after a matching summary are merged into the
previous event instead of shown as separate rows:

```text
[14:06:58.892] INFO (#147):
  {
    messageKey: "http.request",
    requestId: "716d1e62-46a1-46c0-9099-e939a2e4fbb0",
    statusCode: 200,
    durationMs: 96,
  }
```

Inline `key=value` and logfmt-style tokens in the displayed message are also
parsed as properties. Quoted values such as `service="api server"` are supported
for filtering and source promotion. Flat single-line JSON objects are parsed as
properties too.

Property filters support exact values and key existence. Use `key=value` or
`key` to show matching rows, and `key!=value` or `!key` to hide matching rows.
The details pane can prefill these filters from the selected event property.
The active text search is highlighted in visible log-row messages.

Display fields are session-local property keys rendered as stable columns after
the parsed message. The picker lists observed property keys from the retained
buffer. Column widths are based on the longest visible value for each selected
field, capped for long values, and rows that do not have a selected property
show `-` in that column. Only the message column wraps.

## Development

Enter the Nix development shell to get Rust, a native linker, and the release
tools used by this repository:

```sh
nix develop
```

Run tests:

```sh
cargo test
```

Check compilation:

```sh
cargo check
```

Build the debug binary:

```sh
cargo build
```

The debug binary is written to:

```sh
target/debug/loggle
```

## Release

Before the first public release:

- Create or confirm the `maximilianpw/homebrew-tap` GitHub repository.
- Add a GitHub Actions secret named `HOMEBREW_TAP_TOKEN` to this repository.
  The token needs write access to `maximilianpw/homebrew-tap`.
- Add a GitHub Actions secret named `CARGO_REGISTRY_TOKEN` to this repository.
  The token needs permission to publish the `loggle` crate on crates.io.

For each release:

Update `version` in `Cargo.toml`, then run the local checks:

```sh
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo publish --locked --dry-run
```

Commit the release, push it to `main`, then push a matching semver tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

Pushing the tag runs the generated `cargo-dist` release workflow. It builds
Linux and macOS archives, creates the GitHub Release, publishes the Homebrew
formula to `maximilianpw/homebrew-tap`, publishes the crate to crates.io, and
renders the release body with install commands.

Crates.io versions are permanent: a published version cannot be overwritten.
