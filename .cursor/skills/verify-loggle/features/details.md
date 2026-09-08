# Details

Details lets a user inspect the selected event's parsed source, level, time, message, and properties without leaving the TUI.

## Sub-features

- `details-open` toggles the details pane with `Enter` or the command palette.
- `details-fields` shows `details source=`, `level=`, `time=`, and `message `.
- `details-properties` lists property keys with a `>` marker on the selected row.
- `details-close` hides the pane on a second `Enter`.

## How to get to it (user POV)

- Press `Enter` on a selected log row.
- Press `?`, select `Details`, press `Enter`.
- Press `[` or `]` to move the selected property while details are open.

## Driving it with verify-loggle

Preconditions:

- The fixture session from [replay.md](./replay.md) is healthy.
- Filters are clear.
- Details are closed.

- **Select an error.** Jump to the top, then move to the `request failed` row. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- g g` and then `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- j j j j j j`. The selected row is `request failed`.
- **Open details.** Press `Enter`. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- Enter`. The pane contains `details source=api level=error time=10:00:00.050` and `message request failed`.
- **Read properties.** The same pane contains `> requestId = fixture-failed` or `  requestId = fixture-failed` after `]`. Capture it. Run `.cursor/skills/verify-loggle/scripts/verify-loggle capture details/pane.txt`.
- **Close details.** Press `Enter` again. Run `.cursor/skills/verify-loggle/scripts/verify-loggle keys -- Enter`. The capture no longer contains `details source=`.
- **Proof.** `details/pane.txt` shows both the log list identity (`loggle`, `id=verify-<run>`) and the details lines above.

## Gotchas

- Follow mode keeps the selection on the newest row. `gg` first, then `j`, or the details pane describes the last event.
- The folded API error is one visible row. Opening details on `request failed` is the property-rich event. Opening details on `job insert rejected` shows JSON fields instead.
- A selected-row highlight alone is not proof that details opened. Require the `details source=` line.
