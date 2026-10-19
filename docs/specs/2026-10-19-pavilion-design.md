# Pavilion — design

Status: approved in chat 2026-10-19; amended while planning (columns recomputed each run, read-only investigation
instead of a plan permission mode, tray adds a section).

## Goal

One central board for everything Murim tracks: cycle issues and their PRs, PRs awaiting review, automation runs and
meditations. Each item sits in one column (needs you, working, blocked, queued, watching, done). Pavilion
investigates ready issues on its own, waits for the user to approve the plan before coding, and leaves PR follow-ups
to Tempering as today.

Deterministic rules drive state and dispatch. A small local LLM (Ollama) answers only fuzzy judgement questions, and
Claude checks the local verdict before anything that spends tokens. State is stored locally and re-evaluated only
when an item's inputs change.

Not a daemon: Pavilion runs inside Murim, like Automations (Murim open, Mac awake).

## Columns

Exactly one per item, computed by the pure rule `decide`:

| Column | Source |
|---|---|
| **Needs you** | session `needsInput`; Tempering escalation; `PrFlow::AwaitingReview`/`Opened`; investigation plan awaiting approval; investigation ended without a plan; `Ready` verdict = needs user; review agent draft ready; automation run or meditation `needsInput` |
| **Working** | any live session with status `working`/`starting` (coding, investigation, Tempering turn, review agent, automation, meditation) |
| **Blocked** | a Linear "blocked by" issue not in a completed/canceled/duplicate state; or a confirmed inferred blocker not dismissed |
| **Queued** | issue judged ready, unblocked, no session, waiting for an agent slot |
| **Watching** | PR open, nothing to do (Tempering watching); review PR with no agent activity; issue with idle session and no PR flow pending |
| **Done** | issue completed/canceled or PR merged; collapsed |

Precedence when several apply: Needs you > Working > Blocked > Queued > Watching > Done.

Items:
- Cycle issues from `inbox` (key = Linear issue id), with their session and PR health.
- Review PRs from `reviews` (key = `pr:<owner/repo>#<n>`). Pavilion never starts review agents itself.
- Automation runs and meditations (key = session id), shown only while Working or Needs you.

## Store

`~/.murim/pavilion.json`, written atomically (temp file + rename) on every change. Keyed by Linear issue id:

```
{ items: { <issueId>: {
    ready?: Verdict, blocker?: Verdict,        // judgement answers, each with the fingerprint it was made for
    dismissed: [identifier],                   // inferred blockers the user dismissed
    investigation?: { status: running | planReady | failed | approved, plan } } },
  paused: bool }

Verdict { answer, reason, confidence, by: "local" | "claude", fingerprint }
```

Sessions, Tempering memory and escalations stay in `sessions.json` (source of truth). Pavilion reads them and does
not copy them; its store holds only what exists nowhere else. Records of issues that left the board are pruned after
a successful, non-empty inbox poll.

## Reconcile

A Pavilion thread runs `reconcile` every 60s and whenever poked: after each inbox, PR health and reviews poll, and on
every sessions update (hook events). Each run:

1. Builds every card with the pure rules `decide` (issues), `decide_review` and `decide_agent`. They are cheap and run
   every time; nothing about columns is cached.
2. **Fingerprint** (FNV-1a over issue `updatedAt`, title and description; `updatedAt` is added to the Linear query)
   gates only the judgement questions: a verdict whose fingerprint matches is reused, so the LLM never re-answers an
   unchanged issue.
3. Issues missing a fresh verdict go to the judgement worker (one at a time); the card shows "checking…" in Watching
   until it lands, then a poke reconciles again.
4. Starts queued investigations into free slots (below).
5. Emits `pavilion-updated` with the board when it changed.

A failed poll keeps the last data; the header shows when the board last changed.

## Blockers

The Linear issue query also fetches `inverseRelations { nodes { type issue { id identifier state { type } } } }`;
nodes with `type == "blocks"` are this issue's blockers. A blocker in a completed, canceled or duplicate state no longer
blocks. When the last blocker clears, the fingerprint changes and the item moves on (usually to Queued).

## Judgement

Two questions, the only places an LLM is used:

- `Ready`: is this issue clear enough to investigate, or does it need the user? Input: title, description, latest
  Linear comments. Answer `ready | needsUser`.
- `HiddenBlocker`: does the description or comments say it waits on another issue? Input adds the identifiers and titles of
  other board issues. Answer: one board issue id or `none`; any other id is treated as `none`.

**Local:** `POST <ollama>/api/chat` with `format` = the JSON schema `{answer, reason, confidence: 0..1}`, `stream:
false`, 60s timeout, via `ureq` (already a dependency).

**Claude check:** `claude -p --model haiku` with the same question, the input and the local answer, asked to reply
with the same JSON. The call follows the existing pattern in `repo_match.rs`. Claude's answer wins and the verdict is
stored with `by: "claude"`.
- Always (default): every verdict that changes a column is checked: `Ready` before any investigation spawns,
  `HiddenBlocker` before a blocker is shown.
- Low confidence only: checked only when local confidence < 0.7.
- Off: the local verdict is final.

**Fallback:** if Ollama is unreachable or returns unparseable output, Pavilion asks Claude directly and shows "local
model offline" in the header. If Claude also fails, the item stays in its column and the question is retried on the
next changed fingerprint or after 10 minutes, whichever comes first.

## Dispatch

- **Slots:** max agents (default 10) counts every live session: coding, investigation, Tempering, review, automation
  and meditation. Tempering resumes of existing sessions never wait for a slot.
- **Queue order:** Linear priority (urgent first, no priority last), then oldest `updatedAt`.
- **Investigation** (when Auto-investigate is on, Pavilion is not paused and the agent is Claude Code): a session
  keyed `investigation:<issueId>` (like the meditation/automation prefixes) in the repos folder, no PR flow. It is
  read-only the same way as the repo-pick session: hooks via `--settings`, `Edit,Write,NotebookEdit` disallowed, the
  user's default permission mode. Its prompt has the issue, the repo list and Murim's repo guess, and asks for the
  plan as the final message ending with `MURIM_REPO: <repo>`.
  - Stop with a valid `MURIM_REPO` line: the repo is recorded (as a manual pick, so coding starts in its worktree),
    the plan (marker removed) is stored, status `planReady` → Needs you, plus a notification. An empty plan → `failed`.
  - Stop without a valid marker: the agent is asking something; the session is marked Needs input → Needs you, and
    the card opens the investigation terminal.
  - Session ended (crash, app restart) while `running` → Needs you "investigation ended without a plan".
- **Approve plan:** stores the (possibly edited) plan with status `approved`, then calls the existing `start_task`.
  `start_task` appends an approved plan to the issue text, so it also survives the repo-pick path. The investigation
  session is forgotten. Approved with no session yet shows Working "starting agent" (never re-queued).
- **Pause:** stops new investigations and queue starts; Tempering and running sessions continue.

## UI

- **Rail:** **Pavilion** icon at the top; view `"pavilion"` in `localStorage murim.view`, default when unset.
  Count badge = number of Needs you items.
- **Main area:** full-width board, no sidebar list. Columns in order: Needs you, Working, Blocked, Queued, Watching;
  Done collapsed at the bottom. Empty columns shrink to their header.
- **Header:** `n/<max> agents`, last update, "local model offline" when applicable, Pause toggle.
- **Card:** `StateIcon`, `PrIcon`, identifier and title, repo, priority, and a reason line saying why it is in its
  column (e.g. "asks: which auth provider?", "CI failing: lint (attempt 2/3)", "blocked by MUR-41", "inferred blocker:
  waits on API migration", "plan ready"). A small *local* / *Claude-checked* badge when a verdict decided the column.
- **Inline actions:** **Approve plan** / **Edit** on plan cards; **Dismiss** on inferred blockers (adds to
  `dismissedBlockers`); **Start now** on Queued (may exceed the agent limit).
- **Click:** navigates to the existing pane through the existing `navigate` event (Tasks, Reviews, Automations,
  Meditations). Edit opens the plan (`Markdown.tsx` preview + textarea) with Approve.
- **Menu bar:** the tray keeps its sections and adds **Plans to approve**; clicking one opens Pavilion.

## Settings

New **Pavilion** section:
- Ollama URL (default `http://localhost:11434`) and model (default `qwen3:4b`), **Test** button (one tiny chat call).
- Max agents at once (default 10), shown under Settings → AI.
- Auto-investigate (default on).
- Claude check: Always / Low confidence only / Off (default Always).

## Testing

- `decide`: every column and the precedence order.
- Fingerprint: stable across identical inputs, changes when any input changes.
- Verdict cache: hit on same fingerprint, miss on change.
- Slot accounting and queue order.
- Ollama response parsing: valid, malformed, out-of-range confidence, unknown blocker id.
- One check with a stubbed Ollama endpoint end to end through judgement.
- Manual: run the app with two issues where one blocks the other; complete the blocker; watch the other investigate
  and land in Needs you with a plan.

## Out of scope

Background daemon; Pavilion starting review agents; investigating backlog issues outside the cycle; drag-and-drop
between columns.
