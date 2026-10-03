# Search

Search lets a user filter visible rows by text, jump between matches, and restore the unfiltered list.

## Sub-features

- `search-open` opens the `/` prompt from the keyboard and from the command palette.
- `search-match` narrows `visible` to rows whose raw line, message, source, or properties contain the query.
- `search-empty` keeps the TUI up with `visible 0` when nothing matches.
- `search-clear` removes the text filter.

## How to get to it (user POV)

- Press `/` in the TUI, type a query, press `Enter`.
- Press `?`, select `Search`, press `Enter`, type a query, press `Enter`.
- Press `n` or `N` after a query is set.
- Press `c` to clear search with the other filters.

## Driving it with verify-loggle

Preconditions:

- The fixture session from [replay.md](./replay.md) is healthy.
- Filters are clear (`search=-` in the footer).

- **Open prompt.** Press `/`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- /`. The footer shows `/` and not `filters source=`.
- **Match.** Type `job-001` and apply. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- job-001 Enter`. The header shows `visible 1` and the list contains `jobId=job-001`. It does not contain `job-002`.
- **Capture match.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle capture search/match.txt`. The capture still shows `loggle` and `id=verify-<run>`.
- **Empty.** Clear then search for `volcano`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- c / volcano Enter`. The header shows `visible 0`.
- **Palette entry.** Clear, open the palette, run Search. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- c '?' Enter job-001 Enter`. The dialog title `Commands` appears after `?`. After `Enter` twice plus the query, `visible 1` returns.
- **Proof.** Capture the match state at `search/match.txt`. Then restore the baseline with `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- c` so later recipes see `visible 12`.

## Gotchas

- `fixture-failed` also matches `requestId=fixture-failed-extra`. Use `job-001` or `23503` for a tight check.
- Text search is substring match. Property `log --property requestId=fixture-failed` is exact. Do not treat those as the same proof.
- At widths under about 160 columns the footer shows `search=~` even when a query is set. Trust `visible` and the rows, or keep the helper pane at 200 columns.
- `/` while a prompt is already open types a slash into the query.
