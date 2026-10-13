# Automations — design

Status: approved in chat 2026-10-13.

## Goal

Scheduled agent runs (like Orca automations): a schedule + a prompt starts a Claude session in a chosen folder.
When a run waits on the user (e.g. confirming an email send), the user is notified and the rail shows a dot.
Supports every-other-week schedules, which Orca lacks.

## Layout

- Rail: **Automations** (clock icon) below Meditations; view `"automations"` in `localStorage murim.view`.
  Rail dot (brand colour, `.dot-req` style) whenever any automation run has status `needsInput`.
- Sidebar: section **Automations** with **+** (creates "New automation", disabled, opens the editor). Rows: name,
  schedule summary (e.g. `Every 2 weeks · Mon 9:00`), next run (relative, e.g. `in 3d`). Disabled rows dimmed.
  Pulsing dot (`.dot-input`) on rows with a run waiting on the user. Selection remembered per mode.
- Main pane:
  - Automation selected: editor — name, prompt (textarea), folder (text + **Choose…**, default = repos folder),
    schedule picker, Enabled toggle, **Save**, **Run now**, **Delete** (confirm inline). Below: **Runs** list
    (start time, status, ✕ delete), newest first; clicking a run opens it.
  - Run selected: top bar (automation name · start time, Stop / Resume) + Terminal, as in Meditations.

## Schedule model

```
Schedule { kind: daily | weekdays | weekly | biweekly | monthly,
           time: "HH:MM",            // local time
           days: [0..6],             // weekly/biweekly, 0 = Sunday; at least one
           start: "YYYY-MM-DD",      // biweekly anchor
           day_of_month: 1..31 }     // monthly
```

- Local time (macOS timezone).
- `biweekly`: fires on the chosen days only in weeks whose index is even relative to the week (Sunday-start)
  containing `start`, and never before `start`.
- `monthly` day 29–31 falls back to the month's last day in shorter months.
- DST: a nonexistent local time (spring forward) fires at the next valid minute; an ambiguous one (fall back)
  fires once, on the first occurrence.
- Pure `next_after(schedule, after: DateTime<Local>) -> DateTime<Local>`; summary text is built in the frontend.

## Running

- Scheduler thread, 30s tick. For each enabled automation: if `next_due <= now`, start a run and advance
  `next_due = next_after(schedule, now)`.
- On launch or after the Mac wakes (tick sees a gap > 2 min): a due time missed by ≤ 24h runs once; older misses
  are recorded as `missed` runs (one record, not one per missed slot).
- Previous run of the same automation still alive at due time → record a `skipped` run, don't start.
- **Run now** starts a run immediately regardless of schedule (still skipped if one is running → error shown).
- Run session id `automation:<auto-id>:<run-id>`; cwd = automation folder (error recorded as `failed` if missing);
  Settings agent + permission mode; prompt as the first message (plus Settings `default_prompt` like tasks);
  Claude hooks via `--settings` (nothing written into the folder). Custom agents get the prompt as last arg.
- Excluded from GitHub polling, Tempering, PR flow, repo-pick and marker logic (like meditations).
- `needsInput` → notification titled `<automation name> needs you`, body = the question/permission text when
  available. Dot clears when status leaves needsInput. Finished runs don't notify.
- Retention: last 20 runs per automation; older runs' sessions are forgotten (PTY killed, buffers dropped).
- Deleting an automation stops and forgets all its runs.

## Storage

`automations.json` in the app config dir:
`[{ id, name, prompt, folder, schedule, enabled, next_due, runs: [{ id, started, status }] }]`.
Run `status`: `running | needsInput | done | failed | missed | skipped`, derived for live runs from the session
status (working/starting → running, needsInput → needsInput, idle/ended → done); `failed` carries an error string.

## Commands

`list_automations`, `save_automation(a) -> Automation` (validates; recomputes `next_due`),
`delete_automation(id)`, `run_automation_now(id) -> run_id`, `delete_run(auto_id, run_id)`.
Event `automations-updated` with the full list on any change.

## Testing

- Rust unit: `next_after` for each kind; biweekly parity across a year boundary and when `start` is mid-week;
  monthly 31 in February/leap year; DST spring-forward/fall-back; missed-run policy (≤24h runs once, older →
  missed); overlap → skipped; retention trims to 20; validation (weekly with no days, bad time, missing name).
- Mock-backend screenshots: list with dot, editor with each schedule kind, runs list, run terminal pane.

## Not in scope

Raw cron; per-automation agent/permission overrides; chaining; running while Murim is closed.
