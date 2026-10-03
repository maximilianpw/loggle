# v2 reconcile demonstration (2026-10-03)

Branch `revive/v2-reconcile`: the four reviewed v2 commits (`d683bfe`,
`21c2047`, `f8c23bf`, `9438d52`) cherry-picked onto `origin/main` (`1da0465`).
This note shows the combined behaviour from a second process, without opening
the TUI, on the repository's own fixture
`fixtures/mixed-service-investigation.log` (18 raw lines, 12 events).

Scope is the bounded-snapshot contract from `plans/README.md`: it does not
cover cursors, wait/follow, or keeping data after a session exits.

## Setup

`loggle` below is `target/nix/debug/loggle` built with `nix develop -c cargo
build --locked`. State is isolated so the user's `~/.local/state/loggle` is
never touched.

```sh
export XDG_STATE_HOME=/tmp/loggle-v2-demo/state
mkdir -p "$XDG_STATE_HOME"
# Session owner: a disposable TUI in tmux (the agent never attaches to it).
tmux new-session -d -s loggle-v2-demo -x 200 -y 40 -e XDG_STATE_HOME="$XDG_STATE_HOME" -- \
  loggle --no-color --id v2demo -- cat fixtures/mixed-service-investigation.log
```

## Locating one request across services (second process)

```text
$ loggle pages
ID	PID	AGE	COMMAND
v2demo	3327589	2s	cat fixtures/mixed-service-investigation.log

$ loggle sources -i v2demo            # main: source discovery, now session-bound
SOURCE	RECORDS
api	5
database	2
minio-init	1
worker	4

$ loggle facets -i v2demo             # v2: bounded facets (excerpt)
level matched=12 eligible=12 window=12/12 shown=2/2
VALUE	COUNT	TYPES
error	3
info	9
property_key matched=12 eligible=12 window=12/12 shown=10/10
VALUE	COUNT	TYPES
requestId	11
...

$ loggle facets -i v2demo --level error --property-key requestId --format jsonl   # last group
{"schema_version":1,"facet":"property_value","property_key":"requestId","available_records":12,"window_records":12,"window_truncated":false,"matched_records":3,"eligible_records":3,"total_buckets":1,"truncated":false,"buckets":[{"value":"fixture-failed","count":3,"value_types":["string","text"]}]}
```

Every error belongs to `fixture-failed`. Following that one request across
api, worker, and database:

```text
$ loggle log -i v2demo -n 10 --property requestId=fixture-failed
[api] 10:00:00.000 INFO request started requestId=fixture-failed method=POST path=/jobs
[worker] INFO job started requestId=fixture-failed jobId=job-001
database | {"level":"error","message":"job insert rejected","requestId":"fixture-failed","errorCode":"23503","table":"jobs"}
[worker] ERROR job failed requestId=fixture-failed errorCode=23503
[api] 10:00:00.050 ERROR request failed
[api] [10:00:00.050] ERROR (#1):
[api] {
[api]   requestId: "fixture-failed",
[api]   statusCode: 500,
[api]   cause: "job insert rejected: missing synthetic parent",
[api] }

$ loggle log -i v2demo -n 10 --service database --property requestId=fixture-failed --format jsonl
{"schema_version":1,"source":"database","timestamp":null,"level":"error","message":"job insert rejected","properties":{"errorCode":"23503","requestId":"fixture-failed","table":"jobs"},"raw":"database | {\"level\":\"error\",...}"}

$ loggle log -i v2demo -n 1 --service api --level error --property requestId=fixture-failed --format jsonl
{"schema_version":1,"source":"api","timestamp":"10:00:00.050","level":"error","message":"request failed","properties":{"cause":"job insert rejected: missing synthetic parent","requestId":"fixture-failed","statusCode":500},"raw":"[api] 10:00:00.050 ERROR request failed\n[api] [10:00:00.050] ERROR (#1):\n[api] {\n..."}

$ loggle log -i v2demo -n 10 --property requestId=fixture    # exact match only: no prefix collision
                                                              # (zero bytes, exit 0)

$ loggle log -i v2demo -n 10 --service worker --text job --property requestId=fixture-success --clean
[worker] INFO job started requestId=fixture-success jobId=job-002
[worker] INFO job completed requestId=fixture-success jobId=job-002
```

The interleaved `fixture-failed-extra` record is never returned for
`requestId=fixture-failed`. The folded seven-line API property block stays
attached to its summary in both the raw and JSONL output.

## Stale page (crashed session)

```text
$ kill -KILL "$(loggle pages --format jsonl | jq -r .pid)"
$ ls $XDG_STATE_HOME/loggle/active-pages $XDG_STATE_HOME/loggle/pages
active-pages: v2demo.json  v2demo.lock
pages:        v2demo.3327589.1791044157419350658.1.log

$ loggle pages
no active loggle pages
$ loggle log -i v2demo -n 5 --property requestId=fixture-failed
error: no log page found for id 'v2demo' at /tmp/loggle-v2-demo/state/loggle/pages/v2demo.log   [exit 1]
$ loggle sources -i v2demo
error: no log page found for id 'v2demo' at ...                                              [exit 1]
$ loggle facets -i v2demo --format jsonl
error: no log page found for id 'v2demo' at ...                                              [exit 1]
```

The leftover data file is not served, and stdout stays empty. Restarting the same ID
reaps the dead generation and serves only the new one:

```text
$ tmux new-session -d -s loggle-v2-demo ... loggle --no-color --id v2demo -- cat fixtures/mixed-service-investigation.log
$ ls $XDG_STATE_HOME/loggle/pages
v2demo.3328756.1791044187674624375.1.log
$ loggle log -i v2demo -n 1 --service worker --property requestId=fixture-failed
[worker] ERROR job failed requestId=fixture-failed errorCode=23503
```

## Session cleanup

```text
$ tmux send-keys -t loggle-v2-demo q        # clean TUI exit
$ ls -A $XDG_STATE_HOME/loggle/active-pages $XDG_STATE_HOME/loggle/pages
active-pages: v2demo.lock                    # the empty lock file stays on purpose (v2 lease)
pages:        (empty)
$ loggle pages
no active loggle pages
$ loggle log -i v2demo -n 5
error: no log page found for id 'v2demo' at ...   [exit 1]
$ rm -rf /tmp/loggle-v2-demo
```

## Observations (not changed here)

- The missing-page error still prints the legacy `<id>.log` path, not the
  generation file. It is accurate as "not found" but names a path that never
  existed in v2.
- Killing the owning terminal (tmux `kill-server`, so SIGHUP) instead of a
  clean `q` leaves the generation data file until the next session with that ID
  reaps it. This belongs to the post-v2 terminal/process hardening follow-up.
