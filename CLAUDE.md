# CLAUDE.md

Loggle is a Rust (edition 2024) terminal log viewer that tails a launched
command or stdin, parses source/level/properties, and renders a filterable
ratatui TUI. User docs: `README.md`. Dev and release: `CONTRIBUTING.md`.

## Module map

Dependencies point downward; lower modules must not import higher ones.

```text
main.rs            clap CLI; dispatches log/pages/sources/facets/run/start/dc
  -> lib.rs        crate root; re-exports the public API used by main.rs
    -> runtime     event loop: input, terminal, keys, start_plan, clipboard
      -> app       App state; visible (VisibleLogView), list_state
        -> buffer  LogBuffer: bounded store, emits BufferChange
          -> model   line parsing into LogEvent / LogProperty / Level
filter (+ workflow), ui (dialog, row, status, text, theme), page_log, config,
commands       shared leaf-ish modules used by runtime/app
perf.rs + bin/loggle-bench.rs   only with the `perf-harness` feature
```

- `ui` reads `app` state to draw; `app` does not depend on `ui`.
- `config` parses `.loggle.toml` into `runtime::StartCommand`/`ReadySpec`.
- `page_log` reuses `buffer`, `filter`, `facet`, and `model` for
  `loggle log`/`sources`/`facets`.

`src/model/` is being split into per-parser submodules. Target layout:
`model.rs` keeps the shared types and dispatch, with `model/compose`,
`model/buildkit`, `model/structured`, `model/json`, `model/inline`, and
`model/block` (plus the existing `model/interpret`). Put new parsing logic in
the matching submodule rather than in `model.rs`.

## Invariants

- Launched children run in their own process group (`setpgid(0, 0)` in
  `pre_exec`, `runtime/input.rs`). Quit interrupts the group and escalates;
  `Drop for Child` always SIGKILLs the whole group, so never leak a `Child`
  expecting the process to outlive it.
- `LogBuffer::push_line` returns a `BufferChange` (`appended`, `removed`,
  `updated` event sequences). `VisibleLogView` applies it to its filter cache incrementally;
  any new buffer mutation must report itself in `BufferChange` or the visible
  rows go stale.
- The page log is best-effort: write failures surface as a notice in the TUI
  and must never terminate the viewer.
- Page ids are claimed atomically by creating the metadata file with
  `OpenOptions::create_new`; `AlreadyExists` means the id is taken.
- Every CLI flag and keybinding is documented in `README.md`; update it when
  behaviour changes.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

CI runs exactly these (tests on Ubuntu and macOS). `src/runtime/tests.rs` is
gated by `cfg(all(test, target_os = "linux"))`, so runtime integration tests
do not run on macOS; rely on CI for them when developing locally on a Mac.
Focused runs: `cargo test --locked <name_substring>`.
