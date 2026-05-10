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
- Inline message fields for adding selected properties such as `requestId` or
  `durationMs` to every matching row
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
captures both stdout and stderr. That prevents Docker Compose or service output
from writing directly over the TUI.

Pipe mode also works:

```sh
docker compose up 2>&1 | loggle
```

When using pipe mode, include `2>&1`; otherwise stderr can bypass Loggle.

## Options

```sh
loggle --buffer-lines 50000 -- docker compose up
loggle --no-color -- docker compose logs -f
```

- `--buffer-lines <N>`: maximum retained lines, default `100000`
- `--no-color`: disables Loggle's source and severity coloring
- `dc`: shortcut for `docker compose up`
- `[COMMAND]...`: optional command to run under Loggle after `--`

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

### Filtering

- `/`: set text filter/search
- `s`: set source/service filter
- `l`: set level filter
- `+`: add a show property filter
- `-`: add a hide property filter
- `c`: clear filters

### Details and Properties

- `Enter`: toggle selected log details
- `[` / `]`: move through properties in the details pane
- `f`: show only rows with the selected property value
- `m`: append the selected property key to displayed log-row messages

### Dialogs

- `M`: open searchable message field manager
- `P`: open searchable property filter manager
- `?`: open/close the command palette
- Message field manager: type to search; `j` / `k`, arrows, `Ctrl-d` / `Ctrl-u` move selection; `Backspace` or `Delete` removes when search is empty; `Esc` closes
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
| `plain output` | `unknown` | inferred or `unknown` | `plain output` |
| `    at handler` after `[api] ERROR failed` | `api` | inferred or `unknown` | `    at handler` |

The `source | message` form matches Docker Compose output. The `[source]
message` form matches concurrently-style named output, including padded names
such as `[backend ] message` and colored prefixes.

If no supported prefix is found, the source is shown as `unknown`. Loggle does
not infer source names from standalone unprefixed message content because that
identity is lost once an upstream tool merges streams without a marker.
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

Property filters support exact values and key existence. Use `key=value` or
`key` to show matching rows, and `key!=value` or `!key` to hide matching rows.
The details pane can prefill these filters from the selected event property.

Message fields are session-local property keys appended to log rows after the
parsed message as `key=value`. Rows that do not have a selected property omit
that field.

## Development

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
