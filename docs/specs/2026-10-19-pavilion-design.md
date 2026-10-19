# Pavilion — design

Status: approved in chat 2026-10-19.

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

`~/.murim/pavilion.json`, written atomically (temp file + rename) on every change:

```
{ items: { <key>: {
    column, fingerprint,
    blockedBy: [issueId],           // Linear relations
    inferredBlockers: [{ issueId, reason }],
    dismissedBlockers: [issueId],
    verdicts: { ready?: Verdict, hiddenBlocker?: Verdict },
    investigation?: { sessionId, plan, status: running | planReady | failed | approved },
    updatedAt } },
  paused: bool }

Verdict { answer, reason, confidence, by: "local" | "claude", fingerprint }
```

Sessions, Tempering memory and escalations stay in `sessions.json` (source of truth). Pavilion reads them and does
not copy them; its store holds only what exists nowhere else.

## Reconcile

`pavilion::reconcile(app)` runs after each inbox poll (60s), PR health poll (60s), reviews poll (120s), and on
hook events (session status changes). For each item:

1. **Fingerprint:** a hash of data already fetched: issue `updatedAt`, description hash, blocker ids and their
   states, PR head sha, check states, latest comment id, session status, PR flow. `updatedAt` is added to the
   Linear issue query.
2. Unchanged fingerprint → keep stored column, skip.
3. Changed → `decide(item, stored) -> (Column, Option<Question>)`. Pure, unit tested like `tempering::decide`.
4. A question with no verdict for the current fingerprint → judgement (below), run on a worker thread; the item keeps
   its previous column until the verdict lands, then reconciles again.
5. Emit `pavilion-updated` with the full board.

A failed poll keeps the last stored state; the header shows "updated <relative time>".

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

- **Slots:** max agents (default 3) counts every live session: coding, investigation, Tempering, review, automation
  and meditation. Tempering resumes of existing sessions never wait for a slot.
- **Queue order:** Linear priority (urgent first, no priority last), then oldest `updatedAt`.
- **Investigation** (when Auto-investigate is on and Pavilion is not paused): a new session kind with id prefix `inv:`
  (like the meditation/automation prefixes) in the repos folder, no PR flow, run with `--permission-mode plan` (a new `PermissionMode::Plan` variant used only by
  investigations, not offered in Settings). Its prompt has
  the issue, the matched repo (or Murim's guess) and asks for a short implementation plan as the final message. On
  Stop the last message is stored as `investigation.plan`, status `planReady` → Needs you. A session that ends
  with an empty last message → `failed` → Needs you ("investigation ended without a plan").
- **Approve plan:** calls the existing `start_task` with the (possibly edited) plan appended as extra instructions;
  the investigation session is forgotten; status `approved`.
- **Pause:** stops new investigations and queue starts; Tempering and running sessions continue.

## UI

- **Rail:** **Pavilion** icon at the top; view `"pavilion"` in `localStorage murim.view`, default when unset.
  Count badge = number of Needs you items.
- **Main area:** full-width board, no sidebar list. Columns in order: Needs you, Working, Blocked, Queued, Watching;
  Done collapsed at the bottom. Empty columns shrink to their header.
- **Header:** `n/3 agents`, last update, "local model offline" when applicable, Pause toggle.
- **Card:** `StateIcon`, `PrIcon`, identifier and title, repo, priority, and a reason line saying why it is in its
  column (e.g. "asks: which auth provider?", "CI failing: lint (attempt 2/3)", "blocked by MUR-41", "inferred blocker:
  waits on API migration", "plan ready"). A small *local* / *Claude-checked* badge when a verdict decided the column.
- **Inline actions:** **Approve plan** / **Edit** on plan cards; **Dismiss** on inferred blockers (adds to
  `dismissedBlockers`); **Start now** on Queued (may exceed the agent limit).
- **Click:** navigates to the existing pane through the existing `navigate` event (Tasks, Reviews, Automations,
  Meditations). Edit opens the plan (`Markdown.tsx` preview + textarea) with Approve.
- **Menu bar:** the tray list is built from Pavilion's Needs you items (one source).

## Settings

New **Pavilion** section:
- Ollama URL (default `http://localhost:11434`) and model (default `qwen3:4b`), **Test** button (one tiny chat call).
- Max agents (default 3).
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
