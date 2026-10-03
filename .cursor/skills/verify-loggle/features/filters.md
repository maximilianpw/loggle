# Filters

Filters let a user narrow the log list by source, level, or parsed property, then clear or undo that change.

## Sub-features

- `filter-source` keeps rows from one source.
- `filter-level` keeps rows at one level.
- `filter-property-show` keeps rows matching `key` or `key=value`.
- `filter-clear` restores `visible 12` on the fixture.
- `filter-undo` restores the previous filter state.

## How to get to it (user POV)

- Press `s`, type a source such as `database`, press `Enter`.
- Press `l`, type a level such as `error`, press `Enter`.
- Press `+`, type `requestId=fixture-failed`, press `Enter`.
- Press `P` to open the property filter manager.
- Press `c` to clear. Press `u` to undo.

## Driving it with verify-loggle

Preconditions:

- The fixture session from [replay.md](./replay.md) is healthy.
- Filters are clear (`source=-  level=-  search=-  props=-`).

- **Source.** Filter to `database`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- s database Enter`. The header shows `visible 2`. The list contains `job insert rejected` and `job inserted`. It does not contain `minio-init`.
- **Clear.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- c`. The header shows `visible 12`.
- **Level.** Filter to `error`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- l error Enter`. The header shows `visible 3`. The list contains `job insert rejected`, `job failed`, and `request failed`.
- **Undo.** Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- u`. `visible 12` returns.
- **Property.** Show `requestId=fixture-failed`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- c + requestId=fixture-failed Enter`. The header shows `visible 5`. The list does not contain `fixture-success` or `job-002`.
- **Manager.** Open property filters. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- P`. The dialog title is `Property filters` and the row contains `requestId=fixture-failed`. Press `Escape`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- Escape`.
- **Proof.** Capture the property-filtered list. Run `.cursor/skills/verify-loggle/scripts/verify-loggle capture filters/property.txt`. Then `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- c`.

## Gotchas

- TUI `+` property filters are exact, like `loggle log --property`. TUI `/` search is not.
- `u` undoes one filter change. `c` clears all filters. They are not interchangeable proofs.
- Compact footers truncate `props=` values. Use `visible` and row text when the footer shows `~`.
- `P` while a prompt is open types `P` into the prompt.
