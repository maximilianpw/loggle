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
- Pinned field columns for keeping selected properties such as `requestId` or
  `durationMs` aligned across every matching row
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
loggle -- docker compose up
```

`loggle dc` is an exact shortcut for `loggle -- docker compose up`. Only bare
`dc` is special; use the `-- docker compose ...` form for other Compose
commands.

The `--` form is recommended because Loggle starts the command itself and
captures both stdout and stderr, so neither Docker Compose nor service output
writes directly over the TUI.

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

Discover the actual source names before filtering (container aliases can differ
from Compose service names):

```sh
loggle sources -i 1
loggle sources -i 1 --source-field service,app
loggle log -i 1 -n 5 --source api --clean
```

`sources` prints sorted `SOURCE` / `RECORDS` columns for the retained page,
using the same parsing as the TUI source manager. Counts are parsed events, not
raw lines or running-process health. An empty page prints only the header;
a missing page reports an error. Repeat custom `--source-field` settings when
querying a session that uses them.

`log --clean` strips ANSI escapes and remaining terminal control characters
from returned lines, converting tabs to spaces without compacting indentation.
It preserves line boundaries and complete matching records. Matching and stored
raw evidence are unchanged; omit `--clean` for the existing raw output. This is
terminal cleanup, **not secret redaction**.

Text filters match the same event fields as TUI search: raw line, parsed
message, source, and property keys/values. Property filters use the same syntax
as the TUI property prompt: `key`, `key=value`, `key!=value`, and `!key`. A
filtered tail returns whole matching records — the header line plus any folded
multi-line property block — and `-n` counts matching records rather than
individual lines.

Filter by severity with `--level` (`fatal`, `error`, `warn`, `info`, `debug`,
`trace`, `unknown`; case-insensitive, `err`/`warning`/`verbose` accepted). It
composes with the other filters:

```sh
loggle log -i 1 -n 5 --level error
loggle log -i 1 -n 5 --level error --service api --property tenantId=tenant-1
```

For machine consumption, `--json` prints one JSON object per record per line
(JSONL) instead of raw lines:

```sh
loggle log -i 1 -n 1 --property requestId=fixture-failed --json
```

```json
{"schema_version":1,"sequence":6,"source":"api","timestamp":"10:00:00.050","level":"error","message":"request failed","properties":{"cause":"job insert rejected: missing synthetic parent","requestId":"fixture-failed","statusCode":500},"raw":"[api] 10:00:00.050 ERROR request failed\n[api] [10:00:00.050] ERROR (#1):\n[api] {\n[api]   requestId: \"fixture-failed\",\n[api]   statusCode: 500,\n[api]   cause: \"job insert rejected: missing synthetic parent\",\n[api] }"}
```

Record fields (`schema_version` 1):

- `schema_version`: always `1` for this shape; incompatible changes bump it.
- `sequence`: the record's event number in the page's retained window, parsed
  at query time. It increases in recording order but may skip values (a folded
  property block consumes one). It is a stable reference across repeated
  queries of the same page until the page log rotates (after roughly
  `--buffer-lines` more lines).
- `source`, `message`: parsed source and message strings.
- `timestamp`: the parsed timestamp string, or `null`.
- `level`: one of the lowercase `--level` names.
- `properties`: parsed properties as an object, first value per key. Numbers
  are JSON numbers when that is lossless (otherwise strings, e.g. `01` or
  out-of-range integers), booleans and `null` are typed, everything else is a
  string.
- `raw`: the whole record — header plus any folded property block — joined
  with `\n`. `--clean` applies to this field only; matching and stored logs
  are unchanged.

With `--json` and no matches, nothing is printed and the exit status is `0`.
Errors are still printed as text to stderr with exit status `1`.
`loggle pages --json` prints one
`{"schema_version":1,"id":…,"pid":…,"started_unix_seconds":…,"command":…}`
object per active page, and nothing when there are none.

Summarize a page before filtering it: `facets` counts records per source,
level, and property key, and, with `--property-key`, per value of one property:

```sh
loggle facets -i 1
loggle facets -i 1 --property-key requestId --json
```

```text
source (12 records, 4 buckets)
  api         5
  worker      4
  database    2
  minio-init  1
...
```

```json
{"schema_version":1,"facet":"property_value","property_key":"requestId","available_records":12,"window_records":12,"window_truncated":false,"matched_records":12,"eligible_records":12,"total_buckets":3,"truncated":false,"buckets":[{"value":"fixture-failed","count":5,"value_types":["string","text"]},{"value":"fixture-success","count":5,"value_types":["string","text"]},{"value":"fixture-failed-extra","count":1,"value_types":["text"]}]}
```

`facets` accepts the same `--source`, `--text`, `--level`, `--property`, and
`--source-field` filters as `log`. As in the TUI facet dialog, each facet
ignores its own filter, so `--level error` narrows the source counts while the
level facet still shows every level. It aggregates the newest `--records`
parsed records and prints at most `--buckets` buckets per facet (most frequent
first); `window_truncated` and `truncated` report when either bound clipped
the result. Text output escapes control characters in values; `--json` prints
one `schema_version` 1 object per facet in `source`, `level`, `property_key`,
`property_value` order.

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

The viewer opens immediately and streams output while dependencies start. The
status line shows startup progress (for example `starting: 1/3 ready; waiting
for db`), and `q` works during startup: already-launched commands are shut down
the same way as a normal quit. A readiness timeout or a dependency exiting
before it is ready closes the viewer and prints the error.

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
loggle sources -i 1
loggle log -i 1 -n 5 --clean
loggle log -i 1 -n 5 --service api --property tenantId=tenant-1
loggle log -i 1 -n 5 --level error --json
loggle pages --json
loggle --id api -- docker compose up
loggle --source-field service,app < app.log
loggle run --name api -- pnpm start --name web -- pnpm dev
loggle start
loggle start libre
```

- `--buffer-lines <N>`: maximum retained lines, default `100000`
- `--no-color`: disables Loggle's source and severity coloring
- `--record <PATH>`: writes every raw incoming line to a session log file
- `--id <ID>` / `-i <ID>` (alias `--page-id`): uses this page ID instead of an
  auto-generated ID
- `--no-page-log`: disables the per-session page log used by `loggle log`/`pages`
- `--source-field <FIELD>`: promotes matching parsed properties to the source
  column when no explicit prefix exists. Repeat it or pass comma-separated
  fields, e.g. `--source-field service,app`
- `pages`: lists active Loggle pages with ID, PID, age, and command
- `pages --json`: prints one versioned JSON object per active page (JSONL)
- `sources -i <ID>`: lists observed source names and record counts for a page;
  accepts `--source-field`
- `log -i <ID> -n <N>`: prints the last `N` raw lines from a tagged Loggle page
  (`-n` / `--lines`, default `100`)
- `log --source <SOURCE>` / `log --service <SERVICE>` / `log -s <SOURCE>`:
  limits page output to a parsed source/service
- `log --text <QUERY>` / `log --search <QUERY>` / `log -t <QUERY>`: limits page
  output to records matching a text query
- `log --property <FILTER>` / `log -p <FILTER>`: limits page output by parsed
  properties. Repeat for multiple required predicates
- `log --level <LEVEL>`: limits page output to records at one severity
  (`fatal`, `error`, `warn`, `info`, `debug`, `trace`, `unknown`;
  case-insensitive)
- `log --json`: prints one `schema_version` 1 JSON record per line (JSONL)
  instead of raw lines; see [Agent Log Access](#agent-log-access)
- `log --clean`: strips ANSI/control codes from printed lines (the `raw` field
  with `--json`)
- `log --source-field <FIELD>`: applies custom source promotion when reading a
  page
- `facets -i <ID>`: counts records per source, level, and property key in a
  tagged Loggle page; accepts the `log` filters `--source`/`--service`,
  `--text`/`--search`, `--level`, `--property`, and `--source-field`
- `facets --facet <FACET>`: prints only this facet (`source`, `level`,
  `property_key`, `property_value`); repeatable, default `source`, `level`,
  `property_key`
- `facets --property-key <KEY>`: also counts the values of this property
  (`property_value`, which requires it)
- `facets --records <N>`: aggregates the newest `N` parsed records, `1`–`100000`,
  default `10000`
- `facets --buckets <N>`: prints at most `N` buckets per facet, `1`–`100`,
  default `20`
- `facets --json`: prints one `schema_version` 1 JSON object per facet (JSONL)
- `dc`: shortcut for `docker compose up`
- `[COMMAND]...`: optional command to run under Loggle after `--`
- `run --name <NAME> -- <COMMAND...>`: launches one or more named commands,
  prefixes each output line with `[NAME]`, and shows them in one Loggle session
- `start [NAME]`: launches commands from `.loggle.toml` in the current
  directory, or from a named config in the Loggle user config directory

Global options go before the command or subcommand. Every subcommand has its
own help, e.g. `loggle run --help`. A command whose first word is a subcommand
name (`run`, `start`, `log`, `pages`, `sources`, `facets`) runs that subcommand; put it
after `--` to run it as a command instead.

## Controls

Press `?` to open the in-app command palette:

![Loggle command palette and help screen](public/help.jpg)

In split panes 48–79 columns wide, the footer keeps `q quit` and `? commands`
visible by abbreviating filter labels (`s`, `l`, `/`, `p`) and their values.

### Navigation

- `j` / `k`: move one line down/up
- `Ctrl-d` / `Ctrl-u`: half-page down/up
- `gg`: jump to top
- `G`: jump to bottom and resume following
- `n` / `N`: next/previous search match
- `Space` or `p`: pause/resume following
- `y`: copy the selected raw log line to the clipboard
- `v`: start visual-line selection; move with `j` / `k`, arrows, `Ctrl-d` /
  `Ctrl-u`, `gg`, or `G`; `y` copies the selected lines and `Esc` cancels

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
- `F`: open searchable filter facets; pick a source or level to filter by it,
  or a property key to drill into its values
- `O`: open observed source status counts

### Details and Properties

- `Enter`: toggle selected log details
- `[` / `]`: move through properties in the details pane
- `f`: show only rows with the selected property value
- `m`: pin the selected property key as a displayed log-row column

### Dialogs

- `M`: open searchable pinned field manager
- `P`: open searchable property filter manager
- `?`: open/close the command palette

In every dialog, `j` / `k`, arrows, and `Ctrl-d` / `Ctrl-u` move the selection
and `Esc` closes. In addition:

- Pinned field manager: type to search; `Backspace` or `Delete` removes the
  selected field when the search is empty
- Property filter manager: type to search; `Enter` edits; `Backspace` or
  `Delete` removes the selected filter when the search is empty
- Filter facets: counts sources, levels, and property keys over the newest
  100,000 retained rows when opened, applying every active filter except the
  facet's own so alternatives stay visible. Type to search; `Enter` on a source
  or level replaces that filter, and on a property key opens its values;
  `Enter` on a value replaces that key's property filters with `key=value`;
  `Backspace` or `Delete` with an empty search returns from values to the
  keys. Facet choices are undoable with `u`
- Command palette: `Enter` runs the selected command

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

BuildKit step headers establish a source for later records with the same step ID,
including `CACHED`/`DONE` lines. Shell pipes inside `RUN` instructions are not
treated as Compose source separators. Steps without an established service use
the fallback source `build`; ambiguous stage names are not guessed as services.

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

A block is folded only once its closing `}` arrives. If it never closes — the
process exits mid-print, another service's output interrupts it, a new summary
line starts, or it exceeds 256 lines or 256 KiB — Loggle gives up on the fold
and shows the buffered lines as ordinary rows instead. No input is hidden; the
summary just does not gain those properties.

Inline `key=value` and logfmt-style tokens in the displayed message are also
parsed as properties. Quoted values such as `service="api server"` are supported
for filtering and source promotion. Flat single-line JSON objects are parsed as
properties too.

Property filters support exact values and key existence. Use `key=value` or
`key` to show matching rows, and `key!=value` or `!key` to hide matching rows.
The details pane can prefill these filters from the selected event property.
The active text search is highlighted in visible log-row messages.

Pinned fields are session-local property keys rendered as stable columns before
the parsed message. Rows that do not have a selected property show `-` in that
column.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and release.
