# Automations Implementation Plan

**Goal:** Fourth rail mode: scheduled Claude runs (presets incl. every-2-weeks), with a notification + rail dot when a run waits on the user.

**Spec:** docs/specs/2026-10-13-automations-design.md (source of truth — read it first).

**Architecture:** New `src-tauri/src/automations.rs` owns `automations.json`, the schedule math (`next_after`) and a
30s scheduler thread. A run is an ordinary session in `sessions.rs` keyed `automation:<auto-id>:<run-id>`, started
like a meditation (hooks via `--settings`, nothing written into the folder) but with the prompt as the first message.
Frontend adds a rail icon, a sidebar list, an editor + runs pane, and a run terminal pane.

## Global constraints

- Terse style matching the codebase. Never add Claude attribution. Do NOT commit (coordinator commits).
- Backend may add exactly one dependency: `chrono` (default features; needs `Local` + `serde`). No other new deps.
  Frontend: no new npm deps.
- Wire format camelCase (`#[serde(rename_all = "camelCase")]`), times as unix seconds (`u64`/`i64`).

## Shared contract

```ts
type Schedule = { kind: "daily" | "weekdays" | "weekly" | "biweekly" | "monthly"; time: string /* "HH:MM" */;
  days: number[] /* 0=Sun..6=Sat, weekly/biweekly */; start: string /* "YYYY-MM-DD", biweekly */; dayOfMonth: number /* 1..31, monthly */ };
type AutoRun = { id: string; started: number; status: "started" | "failed" | "missed" | "skipped"; error?: string | null;
  sessionId: string | null /* set when status = started */ };
type Automation = { id: string; name: string; prompt: string; folder: string /* "" = repos folder */;
  schedule: Schedule; enabled: boolean; nextDue: number | null /* null when disabled */; runs: AutoRun[] /* newest first, ≤ 20 */ };
```

Live run status is derived in the frontend from `sessions[run.sessionId].status` (starting/working → running,
needsInput → waiting on you, idle/ended or session gone → done). Stored statuses are only the four above.

Commands: `list_automations() -> Automation[]`, `save_automation(automation: Automation) -> Result<Automation>`
(new when id is "" → backend assigns id; validates: name non-empty, time HH:MM, weekly/biweekly need ≥1 day,
biweekly needs a valid start date, monthly 1..31; recomputes nextDue), `delete_automation(id)` (stops + forgets all
run sessions), `run_automation_now(id) -> Result<String /* run id */>` (Err "Already running" if the previous run's
PTY is alive), `delete_run(autoId, runId)` (forget its session).
Event `automations-updated` with `Automation[]` after any change (including scheduler-started runs).

## Task A — backend (src-tauri)

Files: `Cargo.toml` (+chrono), `src/automations.rs` (new), `src/sessions.rs`, `src/lib.rs`, `src/github.rs` /
`src/tempering.rs` only if `sessions::no_pr_flow` needs widening.

1. Schedule + pure `next_after(&Schedule, after: DateTime<Local>) -> Option<DateTime<Local>>` (None only for invalid
   schedules). Rules from the spec: biweekly week parity relative to the Sunday-start week containing `start`, never
   before `start`; monthly 29–31 clamps to last day; DST gap → next valid minute, DST overlap → earliest. Make it
   testable with a fixed timezone (generic over `TimeZone`, tests use `chrono::FixedOffset` and for DST a hand-rolled
   approach or `Local` guarded so tests don't depend on the machine's zone — no chrono-tz dependency).
2. Storage: load/save `automations.json` in `settings::dir`, `Mutex` state managed by Tauri, emit `automations-updated`.
3. Scheduler thread (started from `lib.rs` setup), 30s tick: for enabled automations with `nextDue <= now`:
   - if due was missed by > 24h (launch/wake) → push one `missed` run; else start a run (or `skipped` if previous
     run's PTY alive); then `nextDue = next_after(now)`. Pure decision helper `decide(due, now, alive) -> Action`
     for testing.
   - Retention: keep 20 newest runs; `sessions::forget` the dropped ones' sessions.
4. sessions.rs: generalize the meditation path so `automation:` ids also get hooks via `--settings`, cwd = the
   session's worktree (the automation folder; `""` → repos_root), and skip PR flow / repo-pick / markers /
   Tempering in `on_hook` (likely widen `is_meditation` usage to an "agent-only" predicate; keep meditation-only
   bits like `--add-dir`/artifacts prompt for meditations only). `start_automation_session(app, id, name, folder,
   prompt)` with the prompt (+ `"\n\nInstructions:\n" + default_prompt` when non-empty, as start_task does) as the
   first message. SessionInfo.name = automation name. Missing folder → Err → run recorded `failed` with the error.
   Notification for needsInput on automation sessions: title `"<name> needs you"`.
5. Tests: next_after per kind; biweekly across year boundary and mid-week start; monthly 31 in Feb (leap and not);
   DST; decide() (≤24h runs, >24h missed, alive → skipped); retention trims to 20; validation errors.
6. `cargo test` + `cargo clippy --all-targets` clean for new code.

## Task B — frontend (src)

Files: `types.ts` (types above + `runStatus(run, sessions)` + `scheduleText(s)` e.g. `Every 2 weeks · Mon, Thu 9:00`,
`Weekdays 8:30`, `Monthly · day 1 9:00`), `Icons.tsx` (ClockIcon, same stroke style), new `AutomationList.tsx`,
new `AutomationPane.tsx` (editor + runs list) — run terminal view: reuse the session view from `MeditationPane.tsx`
(export/reuse rather than copy), `App.tsx`, `App.css`.

1. Rail button below Meditations with `.dot-req`-style dot when any session id starting `automation:` has status
   `needsInput`. View `"automations"` persisted; selection `{kind:"automation"|"run", id}` kept per mode.
2. Sidebar: section label "Automations" + `+` (same `.btn ghost new` style as Sessions' +), rows: name, schedule text,
   next run relative (`in 3d`, `in 2h`, `—` when disabled); disabled rows dimmed; pulsing `.dot-input` when a run is
   waiting. Empty: "Nothing yet.".
3. Editor: name, prompt textarea, folder input + Choose… (dialog `open({directory:true})`, placeholder
   "Repos folder"), schedule picker: kind select; time `<input type="time">`; day toggles (S M T W T F S) for
   weekly/biweekly; start `<input type="date">` for biweekly; day-of-month number for monthly; Enabled checkbox;
   Save (shows backend validation errors), Run now, Delete with inline "Delete? Yes / No". New automation (+) creates
   a draft in the pane, saved via save_automation with id "".
4. Runs list under the editor: start time, derived status (running / waiting on you (brand colour) / done / failed
   with error tooltip / missed / skipped), ✕ delete_run; clicking a run with a session opens the run pane (top bar:
   automation name · start time, Stop/Resume, Terminal).
5. Mock check: extend scratchpad `preview2/mock.js` (it already has `click:<css>@<n>`, `med`, `dbl:` steps) with 3
   automations (one biweekly with a needsInput run session, one disabled, one monthly) and handlers for all
   commands; run `pv.sh`, screenshot with `shot.sh`: list + rail dot, editor for biweekly and monthly, runs list,
   run pane, validation error. `pnpm exec tsc --noEmit` clean.
