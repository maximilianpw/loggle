# Loggle — Elixir first milestone

A small terminal log viewer for **one or two named shell commands**. This branch
replaces the Rust application; the complete Rust version remains in Git at
[`ca03f94`](https://github.com/maximilianpw/loggle/commit/ca03f94c2693726034497e05cdc6435a55ad86ef).
This is not feature parity, and no new release has been published. Existing
Homebrew/crates.io releases are the old Rust application.

## Try it

Install Elixir 1.14+ / Erlang OTP 25+ and a C compiler (`apt-get install elixir
erlang-dev build-essential` on Debian 12), then:

```sh
mix local.hex --force
mix deps.get --check-locked
bin/loggle api='while :; do echo "INFO request completed"; sleep 1; done' \
  worker='while :; do echo '\''{"level":"warn","message":"retrying job"}'\'' >&2; sleep 2; done'
```

The development launcher runs from this checkout. Run the packaged launcher from
your project's directory; commands inherit that directory and environment.
Names are unique, 1–12 ASCII letters/digits/underscore/hyphen. Each argument after
the name is **shell syntax**, run with `/bin/sh -c`; quote it as one argument.
Children have closed input (`/dev/null`), not an interactive terminal.

Keys: **q** or **Ctrl-C** quits; **p/Space** pauses/follows; **j/k** scrolls;
**G** follows the tail; **s** cycles named sources; **/** edits a case-sensitive
text filter, **Enter** applies, **Esc** cancels; **c** clears filters. Ctrl-C
also quits from the filter prompt. Pause freezes the end position, not ingestion:
old rows still expire and an old paused view can become empty. Narrow terminals
clip the screen; resize to at least 80×12 for all controls.

Stdout (`out`) and stderr (`err`) retain separate physical-line framing and named
command identity. Single-line JSON objects extract `level`/`severity` and
`message`/`msg`; other lines display as plain text with basic severity detection.
Statuses remain visible when a command exits. Commands **never auto-restart**.
Non-ASCII bytes/control bytes display as `?` in this first milestone, preventing
terminal escape injection without needing Unicode cell-width heuristics.

## Design and limits

Elixir is the application language. ETS is Erlang's built-in in-memory table; it
holds the recent log tail. There is no web server, cluster, per-line process,
or giant GenServer. A small C **external executable**, not a NIF loaded into
the runtime, owns the Unix operations Elixir's normal Port API does not supply:
demand-driven separate pipes, process groups, and saved/restored terminal modes.

* Each command has **one outstanding read of at most 4096 bytes**. The terminal
  has one outstanding reply of at most 64 input bytes plus its dimensions.
  Reads are requested at 50ms screen ticks (roughly 80KiB/s per command).
  The worst input batch is two 4096-byte chunks, at most 8192 newline records.
* Overload **backpressures the commands through bounded OS pipes**; no unbounded
  BEAM mailbox, log queue, disk spool, or claim to lossless recording. Slow
  consumption can slow your application. Quit does not drain unread output.
* Lines retain at most 4096 bytes, discard the rest until newline, and append
  `[truncated]`. Each of the four streams has its own bounded partial line.
* ETS retains at most **2000 records AND 8MiB accounted serialized payload**,
  evicting the oldest. The header reports eviction. This is not a total RSS cap:
  the Erlang runtime, ETS bookkeeping, one bounded batch and viewport copies add
  overhead. Filters and pause never expand retention.
* Quit closes all command ports concurrently. Helpers signal the entire owned
  session's process group with INT, then TERM after 200ms, then KILL after another
  200ms. Pipe EOF also triggers cleanup if the Elixir owner or VM dies. Normal
  shell exit cleans remaining group members too. Terminal settings are restored
  by the terminal helper on pipe EOF, independent of the application.
* Descendants that deliberately detach with `setsid`/change process groups are
  outside this guarantee, as are remotely created resources (e.g. containers).
  Killing the helper itself with SIGKILL cannot run cleanup. Unix uninterruptible
  I/O may delay process death. These are not cgroup/container containment claims.

Deferred: multiline/property views, Compose source inference, TOML/readiness,
recording, page logs, clipboard, Unicode/color styling, Windows and automatic
publishing. `fixtures/` and `public/` remain useful historical references; the
old screenshots are **not** the new UI.

## Tests and local runtime-bundled package

```sh
mix format --check-formatted
mix test --warnings-as-errors
MIX_ENV=prod mix release --overwrite
python3 test/pty_test.py _build/prod/rel/loggle/bin/loggle
mkdir -p /tmp/loggle-package
tar -xzf _build/prod/loggle-0.2.0-dev.tar.gz -C /tmp/loggle-package
/tmp/loggle-package/bin/loggle --help
```

The tarball contains Erlang/Elixir, Jason, and the native helper: **no system
Elixir installation is required to run it**. The `bin/loggle` launcher forwards
arguments to the release runtime. Build on the target OS/architecture/libc;
this is not a cross-platform binary. **Verified locally: Debian 12, Linux x86-64,
Elixir 1.14 / OTP 25.** macOS, ARM and the optional Nix shell are not verified.
System libc/libstdc++/ncurses dependencies still apply to the bundled runtime.
The launcher also uses standard Unix shell utilities (`ps`, `tr`, `cut`, etc.).
The generic `mix release` output mentions daemon commands; use the custom
`bin/loggle NAME=COMMAND` interface instead. No distribution node is started.

CI only builds/tests on Linux; the Rust/Cargo/Homebrew publishing workflows have
been removed. Creating a package does not publish it. `.agents/setup` prepares
an orb; `nix develop` is an optional development shell, not a release builder.

Authoritative design references: [Elixir Port](https://hexdocs.pm/elixir/Port.html),
[Mix releases](https://hexdocs.pm/mix/1.14/Mix.Tasks.Release.html),
[erlexec](https://github.com/saleyn/erlexec), [Exile](https://github.com/akash-akya/exile).
erlexec pushes output messages without demand; Exile has bounded reads but only
direct-PID termination. Neither alone meets both milestone guarantees.
