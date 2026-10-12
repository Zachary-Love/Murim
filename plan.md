# Murim — Plan

One pane of glass for AI feature development. Linear issues + GitHub PRs in one inbox,
one button to put Claude Code on a task, and deterministic flows that babysit the PR to merge.

**Secret sauce:** deterministic code does the watching (polls, checks, rules). Claude is only
invoked when there is real work to do.

## The successful run

1. Open Murim → see my current-cycle Linear issues, grouped by repo.
2. Hit **Start** on a task → Claude Code begins work in its own worktree, in the background.
3. Get pinged only if Claude has a question → open the task, answer in the real Claude Code TUI, close it.
4. Claude finishes → quality pass → PR opened.
5. Turn on **Tempering** → Murim watches CI / reviews / comments and invokes Claude only when something needs fixing.
6. PR is green, approved, no unresolved threads → "Ready to merge".

## Decisions made

| Area | Decision | Why |
|---|---|---|
| Form factor | Local desktop app, **Tauri** (Rust core + web UI) | Runs against local repos/worktrees, reuses local logins |
| Running Claude | Real interactive **`claude` CLI in a hidden PTY** (`portable-pty`), rendered with **xterm.js** on demand | Real Claude Code experience when intervening; uses the user's own subscription (allowed: unmodified CLI); no Node sidecar |
| Agent status | **Claude Code hooks** → Murim's local HTTP server | Deterministic status; no terminal-output scraping |
| Isolation | One **git worktree per task**, branch = Linear `issue.branchName` | Parallel tasks; branch name auto-links PR to Linear |
| GitHub auth | Reuse `gh auth token` | No OAuth flow to build |
| Linear auth | Personal API key, stored in OS keychain (`keyring` crate) | Simplest; OAuth later if ever multi-user |
| Updates | **Polling** (no webhooks — no public URL locally) | GitHub ~5k pts/hr, Linear 2.5k req/hr is plenty |
| Caffeine mode | macOS: spawn `caffeinate -i -w <murim pid>`; others: `keepawake` crate | Keeps flows running; dies with Murim. Lid-closed on battery still sleeps |
| Babysitter name | **Tempering** | "Temper this PR" — harden it until it can merge |
| Quality pass | **ponytail** plugin (`DietrichGebert/ponytail`, MIT) — `/ponytail-review` before PR | Code quality before human review |
| Issue → repo | Issue label = **bare repo name**, repos live under one root (default `~/Documents/GitHub`). Resolve: (1) exact/normalized name match → (2) **local semantic search**: embed labels and repos (name + README head) with `fastembed` (Rust, ONNX, small model e.g. bge-small), match above a similarity threshold → (3) no match → ask Claude (`claude -p --model haiku`) with the label, issue title, and repo list → (4) user can always override from a repo picker. Confirmed label → path is remembered | Deterministic/local first; Claude only on a miss, once per label |
| Permissions | Per-task choice at Start (default from settings): **Normal** (user's Claude settings) · **Allowlist** (Murim-provided) · **Skip** (`--dangerously-skip-permissions`) | Skip shows a warning; worktree isolation limits but doesn't eliminate the blast radius |
| Worktree cleanup | Delete worktree + local branch automatically once the PR is **merged** | Keeps disk and `git worktree list` clean |
| PR creation | **Claude opens the PR** (writes title + description) | Better descriptions than a template |
| Review replies | Claude replies **as the user**, per-PR mode: **Off / Terse / Full** | Terse = ≤5 words ("Fixed", "Looking into it") |

UI vibe: Orca (`stablyai/orca`, MIT) — sidebar + panes — but organized **by repo**.
Orca pattern worth stealing: one status store owned by the backend; every view just renders it.

## Architecture

```
┌──────────────────────── Tauri app ────────────────────────┐
│ UI (React + Vite + xterm.js)                              │
│   Inbox (by repo) · Task detail · Terminal pane · Settings│
│            ▲ Tauri events (state snapshots)  │ commands   │
│ ───────────┼─────────────────────────────────▼─────────── │
│ Rust core                                                 │
│   store      – single source of truth, persisted JSON     │
│   linear     – poll current-cycle issues (GraphQL)        │
│   github     – poll PR health (GraphQL via gh token)      │
│   sessions   – worktree + PTY lifecycle for `claude`      │
│   hooks      – localhost HTTP server receiving CC hooks   │
│   tempering  – rules: health state → action               │
│   caffeine   – keep-awake toggle                          │
└───────────────────────────────────────────────────────────┘
        │ PTY                       ▲ curl POST (hooks)
        ▼                           │
   claude (interactive CLI) in <repo>/.worktrees/<issue-id>
```

### Task state (derived, not stored as free-form)

```
Todo ─Start─▶ Working ⇄ NeedsInput
                 │
                 ▼ (Stop hook + no open question)
             Idle/Done ──PR detected──▶ PR: Checking
                                         ├─ CI failing
                                         ├─ Changes requested
                                         ├─ Unresolved comments
                                         ├─ Awaiting review
                                         └─ Ready to merge
```

Inbox shows the most urgent first: NeedsInput > PR problems > Awaiting review > Working > Todo.

### Hook wiring

Murim writes `<worktree>/.claude/settings.local.json` with hooks that run
`curl -s -X POST http://127.0.0.1:$PORT/hook -H "X-Murim-Token: $TOKEN" -H "X-Murim-Task: $TASK" --data-binary @-`

| Hook | Murim status |
|---|---|
| `UserPromptSubmit`, `PreToolUse` | Working |
| `Notification` (permission prompt / idle waiting) | NeedsInput + OS notification |
| `Stop` | Turn ended → check for PR, run post-turn flow |
| `SessionEnd` | Session closed; keep `session_id` for `--resume` |

Server binds 127.0.0.1 only and requires a per-launch random token (any local process could otherwise spoof status).

**Phase 0 findings (spike passed, `spike/`):**
- **Fast NeedsInput:** `PreToolUse` with `tool_name == "AskUserQuestion"` fires instantly and carries
  `tool_input.questions` (question, header, options, multiSelect). `Notification`
  (`notification_type: "permission_prompt"`) arrives ~6s later with no question text — use it only as backup.
- **`session_id`** is on every hook from the first `UserPromptSubmit` — store it there.
- **Typing into the TUI:** text, ~0.5s pause, then `\r`. Option picking = arrow keys (`ESC [ B`) + `\r`.
  This makes inbox answer buttons feasible later (render options from PreToolUse, send keys).
- **Trust dialog** on every new worktree path ("No, exit" is the default — bare Enter quits). Murim
  detects it in PTY output and sends Down + `\r`. `--dangerously-skip-permissions` skips it.
- **Strip `CLAUDE*` / `AI_AGENT` env vars** from the child — inherited ones break transcripts and `--resume`.
- Payload fields differ per event (Notification/SessionEnd lack `cwd`, `transcript_path`) — parse loosely.
- SIGKILL of Murim → no `SessionEnd`; reconcile on startup via `--resume`.

### Linear query

Viewer's assigned issues where `cycle.isActive` and `state.type` not in `completed`/`canceled`.
Duplicates are their own `state.type == "duplicate"` (verified in Linear's schema.graphql) → filter `nin: [completed, canceled, duplicate]`.
Match on `state.type`, never `state.name`.

### PR health (deterministic checks)

One GraphQL query per open PR: `reviewDecision`, `mergeable`/`mergeStateStatus`, `statusCheckRollup`
(+ per-check conclusions), `reviewThreads { isResolved isOutdated }`, CHANGES_REQUESTED reviews.

### Tempering rules

| Trigger | Deterministic first | Then Claude |
|---|---|---|
| CI check failed | Re-run once (`gh run rerun --failed`) if never retried for this SHA | Send failing check name + log tail into the task's session: "fix CI" |
| Changes requested / new unresolved thread | — | Send the thread(s): "fix, then record a reply or a disagreement" (see Review replies) |
| Merge conflict | — | "rebase onto base branch and resolve" |
| All green + approved + 0 unresolved | Mark **Ready to merge**, notify | — |

Guards (the anti-chaos layer):
- One in-flight Claude action per PR; new triggers queue.
- Dedup by (check-run id / comment id / head SHA) — never act on the same event twice.
- Max N Claude attempts per PR per trigger type (default 3) → then flip to NeedsInput.
- Delivery: if the session's PTY is alive, type into it; else `claude --resume <session_id> "<prompt>"`.

### Review replies (Claude proposes, Murim posts)

Claude never posts review replies directly. For each thread it writes one line to
`<worktree>/.murim/replies.jsonl`:

```json
{"thread_id": "...", "action": "reply", "body": "Fixed", "resolve": true}
{"thread_id": "...", "action": "disagree", "reason": "Changing X breaks Y because ..."}
```

On `Stop`, Murim reads the file and enforces the PR's reply mode deterministically:
- **Off** → replies dropped (fixes still pushed).
- **Terse** → body must be ≤5 words, else truncated to a canned reply ("Fixed" / "Looking into it").
- **Full** → posted as-is via `gh api` (as the user's `gh` identity), thread resolved if `resolve`.
- **disagree** → nothing posted; task goes to **NeedsInput** and the UI shows the comment + Claude's
  reason with buttons: *Do it anyway* · *Post Claude's pushback* · *Write my own reply* · *Ignore*.

### Post-turn flow (before PR)

On `Stop` with no PR yet → Murim prompts in-session: run `/ponytail-review`, apply fixes,
then Claude opens a **draft** PR (`gh pr create --draft`) with its own title/description and
`Fixes <ISSUE-ID>` in the body. Murim marks it ready-for-review once CI is green.

## Phases

0. **Spike (throwaway):** Rust spawns `claude` in a PTY in a worktree with hooks pointing at a tiny
   server; confirm Notification fires on a question, Stop on turn end, and typed input from Rust
   answers it. Also confirm `--resume` with an initial prompt. *Everything else depends on this.*
1. **Skeleton:** Tauri + React app, settings (Linear key, repo paths, Linear team → repo mapping), caffeine toggle.
2. **Inbox:** Linear poller → tasks grouped by repo, filtered to current cycle.
3. **Start + sessions:** worktree, PTY, hooks server, status badges, xterm.js pane, OS notifications.
4. **PR health:** link task → PR (by branch), GitHub poller, health badges.
5. **Tempering:** rules + guards above, per-PR toggle.
6. **Quality pass:** ponytail post-turn flow, draft → ready.

### Phase 1 tasks

Layout: Tauri v2 app at repo root — `src/` (React + Vite + TS, pnpm), `src-tauri/` (Rust).

1. **Scaffold** — Tauri v2 + React/TS via pnpm; app name Murim; one window with sidebar (repos, empty) + main pane. `pnpm tauri build --debug` compiles.
2. **Settings** — `src-tauri/src/settings.rs`: JSON at app config dir `{ repos_root, default_permission_mode: normal|allowlist|skip, caffeine }`; Linear API key in OS keychain (`keyring`). Commands `get_settings`, `save_settings`, `set_linear_key`, `has_linear_key`. UI `src/Settings.tsx`. Unit test: settings round-trip with defaults.
3. **Caffeine** — `src-tauri/src/caffeine.rs`: `set_caffeine(bool)` spawns/kills `caffeinate -i -w <pid>` (macOS); state restored from settings on launch. Toggle in the sidebar footer. Check: toggling on shows `caffeinate` in `pgrep`, off removes it.

### Phase 2 tasks

Shared types (serde `camelCase`):
- `Issue { id, identifier, title, url, branchName, stateName, stateType, priority, labels: Vec<String>, teamKey }`
- `Repo { name, path }`, `RepoMatch { path, source: remembered|exact|semantic|claude|manual }`
- `InboxItem { issue: Issue, repo: Option<RepoMatch> }`

1. **Linear** — `src-tauri/src/linear.rs`: `pub fn fetch_cycle_issues(key) -> Result<Vec<Issue>>` (viewer's issues in active cycle, excluding completed/canceled/duplicate). Unit test: response JSON → filtered `Vec<Issue>`.
2. **Repo matching** — `src-tauri/src/repo_match.rs`: `list_repos(root)`, `resolve(labels, issue_title, repos, remembered) -> Option<RepoMatch>` (exact/normalized → fastembed semantic → `claude -p --model haiku`); remembered map `repo_map` in settings. Unit test: exact/normalized + remembered tiers.
3. **Inbox** — `src-tauri/src/inbox.rs` poll loop (60s + manual refresh) → `inbox-updated` event + `get_inbox`, `refresh_inbox`, `set_issue_repo` commands; UI groups by repo in sidebar, task list in main pane, "No repo" group with picker.

### Phase 3 tasks

Worktrees live outside repos: `~/.murim/worktrees/<repo-name>/<IDENTIFIER>`, branch = `issue.branchName`
(reuse if it exists), based on `origin/<default>` (fallback `HEAD`). Hooks use env `MURIM_PORT`,
`MURIM_TOKEN`, `MURIM_TASK` set on the PTY child; `.claude/settings.local.json` added to the repo's
`.git/info/exclude`. Permission modes → flags: normal = none; allowlist = `--permission-mode acceptEdits`
+ `--allowedTools` for git/gh/common build cmds; skip = `--dangerously-skip-permissions`.

Contract between backend and UI (serde `camelCase`):
- `SessionInfo { issueId, worktree, sessionId: Option, status: starting|working|needsInput|idle|ended, questions: Option<json>, ptyAlive, permissionMode }`
- Commands: `start_task(issueId, permissionMode)`, `resume_task(issueId, prompt: Option)`, `stop_task(issueId)`,
  `get_sessions() -> {issueId: SessionInfo}`, `pty_write(issueId, data)`, `pty_resize(issueId, cols, rows)`,
  `pty_snapshot(issueId) -> base64` (ring buffer, last ~1MB)
- Events: `sessions-updated` (full map), `pty-output { issueId, data: base64 }`

1. **Backend** — `sessions.rs` (worktree, PTY, ring buffer, trust-dialog auto-answer, persistence to
   `sessions.json`; on restart all sessions are `ended`/resumable) + `hooks.rs` (127.0.0.1 server, token,
   pure `fn next_status(current, hook_json) -> Status` with unit tests) + OS notification on → needsInput.
2. **UI** — Start button w/ permission-mode picker, status badges, task pane with xterm.js attached
   (snapshot replay + live), input/resize, open Linear URL via `tauri-plugin-opener`.

### Phase 4 tasks

Only tasks with a session are tracked. Token from `gh auth token` (login-shell PATH); owner/name from the
repo's `origin` URL; PR found by `headRefName == branch` (newest). Poll every 60s + on `Stop` hook + manual refresh.

Contract (serde `camelCase`):
- `PrHealth { number, url, title, state: open|merged|closed, isDraft, ci: passing|failing|pending|none,
  failingChecks: [name], unresolvedThreads, changesRequested, conflict, reviewDecision: Option,
  overall: merged|closed|conflict|ciFailing|changesRequested|unresolvedComments|draft|ciPending|awaitingReview|readyToMerge }`
  (`overall` = first matching in that order — pure fn, unit-tested on fixtures).
- Commands: `get_prs() -> {issueId: PrHealth}`, `refresh_prs()`. Event: `prs-updated` (full map).
- On `merged`: kill PTY, `git worktree remove` (no `--force`; dirty → leave it and surface an error), delete local branch.

1. **Backend** — `github.rs` (token, GraphQL, `derive_overall`, poller, merge cleanup).
2. **UI** — PR badge per row (overall, color-coded, links to PR), task pane PR section (checks failing, unresolved count, review state); inbox sort: needsInput > PR problems > awaitingReview > working > rest.

### Phase 5 tasks

Per-task settings persisted with the session: `tempering: bool` (default off), `replyMode: off|terse|full` (default terse).
Tempering acts only when the session is `idle`/`ended` (never while working/needsInput) — that's the one-in-flight guard.
Triggers come from a richer PR fetch: head SHA, failing check runs (name, databaseId, workflow run id), unresolved
non-outdated threads (id, path, line, comment bodies + authors, last comment id), mergeable.

Rules (pure `fn decide(pr, memory) -> Vec<Action>`, unit-tested):
- CI failing on (sha, check) never seen → `Rerun(run id)` once (`gh run rerun <id> --failed`); seen-and-rerun and still failing →
  `Invoke(prompt with check names + last ~80 lines of `gh run view <id> --log-failed`)`.
- New unresolved thread ids / new last-comment ids → `Invoke(prompt listing threads + reply protocol)`.
- Conflict on new sha → `Invoke("rebase onto <base>, resolve conflicts, push")`.
- readyToMerge newly reached → `NotifyReady`.
- Per (PR, trigger kind) more than 3 invokes → `Escalate` (status needsInput + notification), stop acting on that kind.
Memory (handled keys + attempt counts) persisted in sessions.json. Delivery = existing `resume_task(issueId, prompt)`.

Review replies: prompt tells Claude to append to `<wt>/.murim/replies.jsonl` (`{threadId, action: reply|disagree, body|reason, resolve}`);
`.murim/` added to `.git/info/exclude`. On `Stop`, Murim consumes the file: off → drop; terse → body ≤5 words else "Fixed";
full → post as is. Posting via GraphQL `addPullRequestReviewThreadReply` + `resolveReviewThread`. `disagree` → stored escalation.

Contract additions (serde camelCase):
- `SessionInfo` gains `tempering`, `replyMode`, `escalations: [{ id, threadId, path, comment, reason }]`.
- Commands: `set_tempering(issueId, on)`, `set_reply_mode(issueId, mode)`,
  `resolve_escalation(issueId, escalationId, choice: doIt|postPushback|ownReply|ignore, text: Option)`.
- Event: `tempering-log { issueId, at, message }` (what Tempering did, for the UI feed).

1. **Backend** — `tempering.rs` (decide + memory + actions + replies consumer/poster) + github.rs richer fetch.
2. **UI** — Tempering toggle + reply-mode select (row + pane), escalation cards with the 4 buttons (+ text box for own reply), activity feed from `tempering-log`.

### Phase 6 tasks

- Initial prompt (start_task) ends with: "When the issue is fully implemented, end your final message with the line MURIM_DONE."
- On `Stop` (hooks payload has `last_assistant_message`), deterministic:
  - contains `MURIM_DONE` and no quality pass yet → mark `prFlow: reviewing`, resume with: run `/ponytail:ponytail-review`
    on the branch diff, apply its fixes, run the project's tests, commit, push, then open a **draft** PR with
    `gh pr create --draft` (own title + description, body includes `Fixes <IDENTIFIER>`), end with `MURIM_PR_OPENED`.
  - contains `MURIM_PR_OPENED` → `prFlow: opened`, poke the PR poller.
  - neither marker, no PR yet, not already needsInput → status `needsInput` + notify ("Claude stopped — reply needed?").
    (Tempering-invoked turns are exempt: they don't need the marker.)
- **Pre-PR review (supersedes the push/PR step above):** the quality pass commits but does NOT push or open a PR; Claude writes
  `.murim/pr.md` (title line, blank, body with `Fixes <IDENTIFIER>`) and ends with `MURIM_READY` → `prFlow: awaitingReview`
  + notify. The **Review** tab shows the diff vs the merge base (incl. uncommitted edits), commits, and an editable title/body.
  "Push & create PR" (`create_pr`) refuses on uncommitted changes, then `git push -u origin HEAD` + `gh pr create --draft`
  → `prFlow: opened`. "Send to Claude" (`request_changes`) resumes Claude with the feedback; it re-commits, rewrites
  `pr.md` and ends with `MURIM_READY` again. Non-marker turns while awaiting review stay idle.
- PR poller: PR is draft + `ci == passing` + `prFlow == opened` → `gh pr ready <n>` once (`prFlow: ready`), tempering-log it.
- `SessionInfo` gains `prFlow: none|reviewing|awaitingReview|opened|ready` (serde default none). UI: show it as a small step label in row + pane.

Each phase ends with one runnable check (Rust unit test for the rules/state derivation; manual run for UI).

## Open questions

None blocking. Revisit after the Phase 0 spike.

## Out of scope (for now)

Inbox-native answer buttons (answer in the terminal instead), multi-user/team mode, hosted runner,
webhooks, non-Claude agents, Windows/Linux polish.

## References

- Orca ADE — https://github.com/stablyai/orca
- Cyrus (Linear + Claude Code) — https://github.com/cyrusagents/cyrus
- ponytail — https://github.com/DietrichGebert/ponytail
- Claude Code hooks — https://code.claude.com/docs/en/hooks.md
- Claude Code CLI — https://code.claude.com/docs/en/cli-reference.md
- Linear GraphQL — https://linear.app/developers/graphql
- GitHub GraphQL rate limits — https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api
