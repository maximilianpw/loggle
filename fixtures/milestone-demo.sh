#!/bin/sh
# Two-source demo: plain stdout and single-line JSON stderr.
i=0
while [ "$i" -lt 12 ]; do
  i=$((i + 1))
  if [ "$1" = api ]; then
    printf 'INFO GET /api/orders request=%s status=200 duration=12ms\n' "$i"
  else
    printf '{"level":"warn","message":"retrying invoice job %s (database busy)"}\n' "$i" >&2
  fi
  sleep 0.4
done
if [ "$1" = api ]; then sleep 300; else exit 7; fi
