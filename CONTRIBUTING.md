# Contributing to Loggle

## Development

Enter the Nix development shell to get Rust, a native linker, and the release
tools used by this repository:

```sh
nix develop
```

Run the same checks as CI before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

`--all-features` enables the `perf-harness` feature so the `loggle-bench`
binary is compiled, linted, and tested too. The runtime integration tests in
`src/runtime/tests.rs` only run on Linux.

Check compilation or build the debug binary (written to `target/debug/loggle`):

```sh
cargo check --locked --all-targets --all-features
cargo build
```

Run the viewer from the source tree:

```sh
cargo run -- -- docker compose up
```

### Mixed-Service Fixture

`fixtures/mixed-service-investigation.log` is a deterministic, synthetic
scenario of interleaved API, worker, database, and init output. It is not a
recording from a real application and needs no Docker, network, secrets, or
service installations. Replay it locally:

```sh
cargo run -- --id fixture -- cat fixtures/mixed-service-investigation.log
```

While the page remains open, query it from another terminal:

```sh
cargo run -- log -i fixture -n 10 --property requestId=fixture-failed
cargo run -- log -i fixture -n 1 --property requestId=fixture-failed
cargo run -- log -i fixture -n 10 --service database --property requestId=fixture-failed
cargo run -- log -i fixture -n 10 --property requestId=fixture-success
```

Expected results:

- The 18 raw lines parse into 12 events.
- `fixture-failed` selects five API/worker/database events: the job insert
  rejection (`23503`), the worker failure, and API status `500`. The last
  matching record includes all seven lines of the API summary/property block,
  including the synthetic cause.
- The interleaved `fixture-failed-extra` request does not match that exact
  property filter.
- `fixture-success` selects five events for a second `POST /jobs` attempt,
  ending with a database insert, worker completion, and API status `201`.

The API/worker output is bracket-prefixed and the database output is
Compose-style, both existing Loggle formats. The separately sourced
`minio-init` line stands in for one-shot initialization output; it does not
exercise capturing a real `--rm` container. Database request correlation is
synthetic: native database logs do not automatically contain application
request IDs.

The fixture is covered by
`buffer::tests::mixed_service_fixture_preserves_sources_properties_and_raw_summaries`
and
`page_log::tests::mixed_service_fixture_replays_exact_correlations_and_whole_records`,
which check source identity, exact property correlation, raw summary
preservation, whole multi-line page-log records, and exclusion of unrelated
traffic. Run them with:

```sh
cargo test --locked mixed_service_fixture
```

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

## Release

Before the first public release:

- Create or confirm the `maximilianpw/homebrew-tap` GitHub repository.
- Add a GitHub Actions secret named `HOMEBREW_TAP_TOKEN` to this repository.
  The token needs write access to `maximilianpw/homebrew-tap`.
- Add a GitHub Actions secret named `CARGO_REGISTRY_TOKEN` to this repository.
  The token needs permission to publish the `loggle` crate on crates.io.

For each release, update `version` in `Cargo.toml`, then run the local checks:

```sh
cargo fmt --all -- --check
cargo test --locked --all-targets --all-features
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
