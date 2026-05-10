# Loggle

Loggle is a terminal log viewer for local, newline-delimited logs. It is built for
Docker Compose and multi-process development workflows such as:

```sh
loggle -- docker compose up
```

It opens a live-tail TUI, parses source-prefixed log lines, infers log levels,
keeps a bounded in-memory buffer, and provides Vim-style navigation and simple
filtering.

## Usage

From this repository:

```sh
cargo run -- -- docker compose up
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

- `j` / `k`: move one line down/up
- `Ctrl-d` / `Ctrl-u`: half-page down/up
- `gg`: jump to top
- `G`: jump to bottom and resume following
- `/`: set text filter/search
- `s`: set source/service filter
- `l`: set level filter
- `Enter`: toggle selected log details
- `[` / `]`: move through properties in the details pane
- `f`: follow the selected property value
- `+`: add an include property filter
- `-`: add an exclude property filter
- `c`: clear filters
- `n` / `N`: next/previous search match
- `Space` or `p`: pause/resume following
- `?`: open/close the command palette
- Command palette: `j` / `k`, arrows, `Ctrl-d` / `Ctrl-u` move selection; `Enter` runs; `Esc` closes
- `Esc`: close prompt or clear transient mode
- `q`: quit

## Log Parsing

Loggle treats lines in these shapes as source-prefixed logs:

```text
service-name | message
[source] message
```

The first form matches Docker Compose output. The second form matches
concurrently-style named output, including padded names such as
`[backend ] message`.

If no supported prefix is found, the source is shown as `unknown`. Loggle does
not infer source names from unprefixed message content.

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
`key` for includes, and `key!=value` or `!key` for excludes. The details pane
can prefill these filters from the selected event property.

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
