# Pavilion UI (mockup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle Pavilion to the user's "mission control" mockup: a header, four stat cards, a filterable work queue with List/Board views, and a right-hand inspector, all driven by Pavilion's real data.

**Architecture:** The backend adds a few display fields to `Card` (source, stage, role, summary, url, verdict), computed by pure functions in `pavilion.rs`. The frontend gets a pure helper module `src/pavilionView.ts` (filters, counts, labels; checked by a node assert script), a new `src/PavilionInspector.tsx`, and a rewritten `src/Pavilion.tsx`. Styles replace the old `.pv-*` block in `src/App.css` and use Murim's theme variables only, so every Appearance theme still works.

**Tech Stack:** Rust (Tauri 2), React + TypeScript (Vite), node 24 for `scripts/*.check.ts`.

**Spec:** `docs/specs/2026-10-19-pavilion-design.md` (behaviour) plus the visual reference: `docs/specs/2026-10-19-pavilion-ui-mockup.html` and its screenshots `docs/specs/2026-10-19-pavilion-ui-mockup-list.png` and `docs/specs/2026-10-19-pavilion-ui-mockup-board.png`.

## Design decisions (what to take from the mockup, what to ignore)

The user said: "Ignore wording and titles that don't match the app."

**Take:** the layout and hierarchy.
- Header block: an eyebrow line, a big title with an accent dot, a subhead, and controls on the right.
- Four stat cards. The first one (Needs you) is highlighted when non-zero.
- Queue panel: title and count, a List/Board segmented toggle, quick-filter chips with counts, a search box with a `/` hint, filter selects, a Clear link, and a "Showing N of M" line.
- List rows: an ident line (Linear id, PR number, repo, priority badge), a bold title, a coloured reason line, and on the right a status pill, the role, the age and a small pipeline progress bar.
- Board: columns of mini cards.
- Inspector:
  - Caps header, ident line, title, status pill and summary.
  - Tabs.
  - Pipeline stepper.
  - A highlighted "decision" box with actions.
  - An "orchestrator decision" block with confidence and source chips.
  - Linked artifacts.
  - A checks list.
  - Sticky footer actions.

**Ignore:**
- The mockup's own left sidebar: Murim keeps its rail, and the Pavilion view hides Murim's sidebar as it does today.
- "Mission control", "Command deck", "Simulate cycle", "Interactive demo" and "Demo snapshot" badges.
- The Policies modal (Settings → Pavilion already exists), "Local memory", the stage/agent filter selects, "Hold for now", "Verify with Claude", and the mockup's colours and fonts.

**Colours and fonts:** use only Murim theme variables: `--bg --surface --raised --hover --line --line-hi --text --text-2 --muted --dim --brand --brand-dim --on-accent --red --amber --blue --green`, plus `var(--ui-font)` and `var(--mono)`. Tints come from `color-mix(in srgb, var(--x) N%, transparent)`, as App.css already does. No hard-coded hex colours.

**Wording mapping:**

| Mockup | Pavilion |
|---|---|
| Eyebrow "AUTONOMOUS DEVELOPMENT / COMMAND DECK" | "PAVILION" |
| Title "Mission control." | "Pavilion." |
| Subhead | "Every issue, PR and agent in one place." |
| Stat "Needs your input" | "Needs you", sub "N plans to approve" or "nothing waiting" |
| Stat "Agents working" | "Agents working", value = board.working, sub "n coding · n investigating · n reviewing" |
| Stat "Blocked by dependencies" | "Blocked", sub "n inferred" or "Linear relations" |
| Stat "Open work items" | "Open items", sub "Linear + GitHub" |
| Queue title | "Work queue" (List) / "Work by state" (Board) |
| Chips | All · Needs you · Working · Blocked · Queued · Failing CI |
| Filters | search, "All sources" (Linear issues / Review PRs / Agents), "All repositories", Clear |
| Board columns | Needs you · In flight · Queued / blocked · Watching · Done |
| Pipeline | Triage · Investigate · Plan · Implement · Review · Merged |
| Inspector tabs | Overview · Plan (when a plan exists) · Terminal (when an investigation session exists) · Activity (Tempering log) |
| "Human decision required" | "Your decision" |
| "Orchestrator decision" | "Judgement", with "92% confidence" and chips "Local model" / "Claude-checked" |
| Header controls | status pills (local model offline / Linear error / updated time), **Refresh**, **Pause agents** / **Resume agents** |

**Behaviour:**
- A row or card click selects it into the inspector; it no longer navigates.
- The inspector footer has "Open in Tasks / Reviews / Automations / Meditations" (the existing `onOpen(view, target)`) and the card's primary action.
- The first card is selected by default. The List/Board choice persists in `localStorage["murim.pavilion.view"]`.
- Narrow windows: under 1100px wide the inspector becomes an overlay drawer with a close button. Under 900px the stat cards go 2×2.

## Global Constraints

- No new dependencies (Rust or JS).
- Theme: only Murim CSS variables (listed above); no hex colours in new CSS or TSX.
- Existing behaviour must keep working: Approve plan (with edits), Dismiss blocker, Start now, Pause, investigation terminal, navigation to other views, rail badge, menu bar navigate.
- Code style: match the surrounding code (dense one-liners, `// ponytail:` on deliberate shortcuts). Rust tests stay in-file; TS pure logic is checked by `scripts/<name>.check.ts` run with `node scripts/<name>.check.ts`.
- Commit messages: plain sentences (`Pavilion UI: …`), no AI attribution or Co-Authored-By lines.
- Branch: `pavilion` (continue on it).
- Verification: `cargo test --manifest-path src-tauri/Cargo.toml` (one name filter per invocation), `pnpm build`, `node scripts/pavilionView.check.ts`.

## Review Focus

- A card that disappears while selected (merged, issue left the cycle): the inspector must fall back to the first card or an empty state, never crash or show stale data. (Task 4.)
- Approving a plan from the inspector after editing it must send the edited text, and the editor must not lose edits when an unrelated board update arrives. Only a new plan resets it. (Task 3.)
- Filters that hide the selected card: the inspector keeps showing the selected card until another is clicked. The counts in chips and stats always use the unfiltered board. (Tasks 2 and 4.)
- Cards without a Linear part (review PRs, automations, meditations) have no ident, url or stage. Rows, board cards and the inspector must render without empty separators or broken links. (Tasks 3 and 4; Task 2 checks the helpers with such cards.)
- Theme switching (Light / Custom) must keep the new UI readable, since no colours are hard-coded. (Final manual check.)

## File Structure

- Modify `src-tauri/src/pavilion.rs`: `Source`, `Stage`, `Role` enums; new `Card` fields; `stage_of`, `role_of`, `summary_of`; reconcile fills them; tests.
- Modify `src/types.ts`: Card fields and the `PvStage`, `Role`, `Verdict` types.
- Create `src/pavilionView.ts` and `scripts/pavilionView.check.ts`: pure filters, counts, labels.
- Create `src/PavilionInspector.tsx`: the inspector.
- Rewrite `src/Pavilion.tsx`: header, stats, queue (list + board), layout.
- Modify `src/App.tsx`: pass `logs` to Pavilion.
- Modify `src/Icons.tsx`: at most a few small line icons used by the new UI (alert, search, list, columns), drawn with the existing `L`/`S` constants.
- Modify `src/App.css`: replace the `/* Pavilion board */` block.

---

### Task 1: Card display fields (backend)

**Files:**
- Modify: `src-tauri/src/pavilion.rs` (enums near `Action`, the `Card` struct, pure fns after `inv_id`, `reconcile` card building, tests)

**Interfaces:**
- Produces on `Card` (serde camelCase):
  - `source: Source` (`"issue" | "review" | "agent"`, default Issue)
  - `stage: Option<Stage>` (`"triage" | "investigate" | "plan" | "implement" | "review" | "merged"`)
  - `role: Option<Role>` (`"investigator" | "coder" | "reviewer" | "automation" | "meditation"`)
  - `summary: String`, `url: String`, `verdict: Option<Verdict>`
- Produces pure fns: `pub fn stage_of(v: &IssueView) -> Stage`, `pub fn role_of(v: &IssueView) -> Option<Role>`, `pub fn summary_of(desc: &str) -> String`.

- [ ] **Step 1: Write the failing tests** in `pavilion.rs` `mod tests`. They reuse the existing helpers `issue`, `sess`, `pr`, `inv`:

```rust
    #[test]
    fn stage_and_role_follow_the_flow() {
        let i = issue("1");
        let board: HashSet<String> = HashSet::new();
        let r = Record::default();
        let v = |session, invs, p, rec| IssueView { issue: &i, session, inv: invs, pr: p, rec, board: &board };
        let (s, iv) = (sess("1", "working"), sess("investigation:1", "working"));
        let (open, merged) = (pr(Overall::AwaitingReview, PrState::Open), pr(Overall::Merged, PrState::Merged));
        assert_eq!(stage_of(&v(None, None, None, &r)), Stage::Triage);
        assert_eq!(stage_of(&v(None, Some(&iv), None, &inv(InvStatus::Running))), Stage::Investigate);
        assert_eq!(stage_of(&v(None, None, None, &inv(InvStatus::Failed))), Stage::Investigate);
        assert_eq!(stage_of(&v(None, None, None, &inv(InvStatus::PlanReady))), Stage::Plan);
        assert_eq!(stage_of(&v(None, None, None, &inv(InvStatus::Approved))), Stage::Implement);
        assert_eq!(stage_of(&v(Some(&s), None, None, &r)), Stage::Implement);
        assert_eq!(stage_of(&v(Some(&s), None, Some(&open), &r)), Stage::Review);
        assert_eq!(stage_of(&v(None, None, Some(&merged), &r)), Stage::Merged);
        assert_eq!(role_of(&v(Some(&s), Some(&iv), None, &r)), Some(Role::Coder));
        assert_eq!(role_of(&v(None, Some(&iv), None, &r)), Some(Role::Investigator));
        assert_eq!(role_of(&v(None, None, None, &r)), None);
    }

    #[test]
    fn summary_is_first_paragraph_capped() {
        assert_eq!(summary_of("\n\nFirst para\nstill first.\n\nSecond"), "First para\nstill first.");
        assert_eq!(summary_of(""), "");
        let long = "x".repeat(300);
        let s = summary_of(&long);
        assert_eq!((s.chars().count(), s.ends_with('…')), (280, true));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pavilion::`
Expected: compile errors (`stage_of`, `Stage`… not found).

- [ ] **Step 3: Implement.** After the `Action` enum:

```rust
#[derive(Serialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    #[default]
    Issue,
    Review,
    Agent,
}

/// Where an issue is in Murim's flow (the inspector's pipeline).
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Triage,
    Investigate,
    Plan,
    Implement,
    Review,
    Merged,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Investigator,
    Coder,
    Reviewer,
    Automation,
    Meditation,
}
```

Add to `Card`, after `updated_at`:

```rust
    pub source: Source,
    pub stage: Option<Stage>,
    /// who is (or was last) on it
    pub role: Option<Role>,
    /// first paragraph of the issue description
    pub summary: String,
    /// Linear issue or PR url
    pub url: String,
    /// the judgement behind the column, if any (inspector)
    pub verdict: Option<Verdict>,
```

After `inv_id`:

```rust
pub fn stage_of(v: &IssueView) -> Stage {
    let inv = v.rec.investigation.as_ref().map(|i| i.status);
    match v.pr {
        Some(p) if p.state == PrState::Merged => Stage::Merged,
        Some(_) => Stage::Review,
        None if v.session.is_some() || inv == Some(InvStatus::Approved) => Stage::Implement,
        None if inv == Some(InvStatus::PlanReady) => Stage::Plan,
        None if inv.is_some() || v.inv.is_some() => Stage::Investigate,
        None => Stage::Triage,
    }
}

pub fn role_of(v: &IssueView) -> Option<Role> {
    if v.session.is_some() {
        Some(Role::Coder)
    } else {
        v.inv.map(|_| Role::Investigator)
    }
}

/// First non-empty paragraph, capped at 280 chars for the inspector.
pub fn summary_of(desc: &str) -> String {
    let p = desc.split("\n\n").map(str::trim).find(|p| !p.is_empty()).unwrap_or("");
    if p.chars().count() > 280 { format!("{}…", p.chars().take(279).collect::<String>()) } else { p.to_string() }
}
```

In `reconcile`, build the `IssueView` once and reuse it:

```rust
            let v = IssueView { issue: i, session: map.get(&i.id), inv, pr: prs.get(&i.id), rec, board: &board_ids };
            let dec = decide(&v);
```

In the issue `Card { … }` literal, add:

```rust
                source: Source::Issue, stage: Some(stage_of(&v)), role: role_of(&v), summary: summary_of(&i.description), url: i.url.clone(),
                verdict: if dec.action == Some(Action::DismissBlocker) { rec.blocker.clone() } else { rec.ready.clone() },
```

In the review card literal, add `source: Source::Review, stage: Some(Stage::Review), role: map.get(&p.session_id).map(|_| Role::Reviewer), url: p.health.url.clone(),`. In the agent card literal, add `source: Source::Agent, role: Some(if sessions::is_automation(id) { Role::Automation } else { Role::Meditation }),`. Keep `..Default::default()`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all pass, no warnings from pavilion.rs.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/pavilion.rs
git commit -m "Pavilion UI: card source, stage, role, summary, url and verdict"
```

---

### Task 2: Frontend types and pure view helpers

**Files:**
- Modify: `src/types.ts` (Pavilion section)
- Create: `src/pavilionView.ts`
- Create: `scripts/pavilionView.check.ts`

**Interfaces:**
- Consumes: the Card JSON from Task 1.
- Produces:
  - `types.ts`: `PvStage`, `Role`, `Verdict`, and Card fields `source`, `stage`, `role`, `summary`, `url`, `verdict`.
  - `pavilionView.ts`: `Chip`, `CHIPS`, `Filters`, `NO_FILTERS`, `chipMatch`, `filterCards`, `chipCounts`, `reposOf`, `stats`, `prioLabel`, `ago`, `PIPELINE`, `stepOf`, `Tone`, `pill`, `BOARD`, `SOURCE_LABEL`, `ROLE_LABEL`, `VIEW_LABEL`.

- [ ] **Step 1: Types.** In `src/types.ts`, in the Pavilion section, add before `Card`:

```ts
export type PvStage = "triage" | "investigate" | "plan" | "implement" | "review" | "merged";
export type Role = "investigator" | "coder" | "reviewer" | "automation" | "meditation";
export type Verdict = { answer: string; reason: string; confidence: number; by: "local" | "claude"; fingerprint: string };
```

and add to the `Card` type: `source: "issue" | "review" | "agent"; stage: PvStage | null; role: Role | null; summary: string; url: string; verdict: Verdict | null;`

- [ ] **Step 2: Write the failing check** `scripts/pavilionView.check.ts`:

```ts
import assert from "node:assert/strict";
import { filterCards, chipCounts, reposOf, stats, prioLabel, ago, stepOf, pill, NO_FILTERS } from "../src/pavilionView.ts";

const card = (o: any) => ({ key: "k", column: "watching", ident: "", title: "t", repo: "", priority: 0, reason: "", by: null, action: null, view: "tasks", target: "k",
  pr: null, session: null, plan: null, updatedAt: "", source: "issue", stage: null, role: null, summary: "", url: "", verdict: null, ...o }) as any;
const cards = [
  card({ key: "a", column: "needsYou", ident: "MUR-1", title: "Auth middleware", repo: "api", action: "approvePlan", priority: 2 }),
  card({ key: "b", column: "working", ident: "MUR-2", repo: "api", role: "coder", pr: { overall: "ciFailing", number: 7 } }),
  card({ key: "c", column: "blocked", ident: "MUR-3", repo: "web", action: "dismissBlocker" }),
  card({ key: "d", column: "queued", ident: "MUR-4", repo: "web" }),
  card({ key: "e", column: "watching", source: "review", ident: "api#9", repo: "api", role: "reviewer" }),
  card({ key: "f", column: "working", source: "agent", title: "Nightly digest", role: "automation" }),
  card({ key: "g", column: "done", ident: "MUR-5", repo: "api" }),
];
const keys = (v: any[]) => v.map((c) => c.key).join("");

assert.equal(keys(filterCards(cards, NO_FILTERS)), "abcdefg");
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, chip: "working" })), "bf");
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, chip: "failingCi" })), "b");
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, source: "review" })), "e");
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, repo: "web" })), "cd");
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, q: "AUTH" })), "a"); // case-insensitive over title
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, q: "#7" })), "b"); // PR number
assert.equal(keys(filterCards(cards, { ...NO_FILTERS, q: "  " })), "abcdefg"); // blank search = no filter
assert.deepEqual(chipCounts(cards), { all: 7, needsYou: 1, working: 2, blocked: 1, queued: 1, failingCi: 1 });
assert.deepEqual(reposOf(cards), ["api", "web"]); // agent card has no repo
const s = stats({ cards, working: 3, max: 3, paused: false, localOffline: false, updated: 0, error: null } as any);
assert.deepEqual(s, { needsYou: 1, plans: 1, working: 3, max: 3, coding: 1, investigating: 0, reviewing: 0, blocked: 1, inferred: 1, open: 6 });
assert.deepEqual([prioLabel(0), prioLabel(1), prioLabel(2), prioLabel(4), prioLabel(5)], ["", "P0", "P1", "P3", ""]);
const now = Date.parse("2026-10-19T12:00:00Z");
assert.deepEqual(["2026-10-19T11:59:40Z", "2026-10-19T11:58:00Z", "2026-10-19T09:00:00Z", "2026-10-16T12:00:00Z", "", "nope"].map((t) => ago(t, now)), ["now", "2m ago", "3h ago", "3d ago", "", ""]);
assert.deepEqual([stepOf(null), stepOf("triage"), stepOf("merged")], [-1, 0, 5]);
assert.deepEqual(pill(cards[0]), { label: "Plan ready", tone: "amber" });
assert.deepEqual(pill(cards[1]), { label: "CI failing", tone: "red" }); // CI beats "Agent running"
assert.deepEqual(pill(cards[5]), { label: "Agent running", tone: "green" });
assert.deepEqual(pill(cards[6]), { label: "Merged", tone: "green" });
assert.deepEqual(pill(cards[3]), { label: "Queued", tone: "grey" });
console.log("pavilionView ok");
```

Run: `node scripts/pavilionView.check.ts`
Expected: fails (module not found).

- [ ] **Step 3: Implement** `src/pavilionView.ts`:

```ts
// Pavilion's pure view logic: filters, counts and labels (checked by scripts/pavilionView.check.ts).
import type { Board, Card, Column, PvStage, Role } from "./types";

export type Chip = "all" | Exclude<Column, "watching" | "done"> | "failingCi";
export const CHIPS: [Chip, string][] = [["all", "All"], ["needsYou", "Needs you"], ["working", "Working"], ["blocked", "Blocked"], ["queued", "Queued"], ["failingCi", "Failing CI"]];
export type Filters = { chip: Chip; q: string; source: "all" | Card["source"]; repo: string };
export const NO_FILTERS: Filters = { chip: "all", q: "", source: "all", repo: "all" };
export const SOURCE_LABEL: Record<Card["source"], string> = { issue: "Linear issues", review: "Review PRs", agent: "Agents" };
export const ROLE_LABEL: Record<Role, string> = { investigator: "Investigator", coder: "Coder", reviewer: "Reviewer", automation: "Automation", meditation: "Meditation" };
export const VIEW_LABEL: Record<string, string> = { tasks: "Tasks", reviews: "Reviews", automations: "Automations", meditations: "Meditations" };

const failing = (c: Card) => c.pr?.overall === "ciFailing";
export const chipMatch = (c: Card, chip: Chip) => (chip === "all" ? true : chip === "failingCi" ? failing(c) : c.column === chip);

export const filterCards = (cards: Card[], f: Filters) => {
  const q = f.q.trim().toLowerCase();
  return cards.filter((c) => chipMatch(c, f.chip) && (f.source === "all" || c.source === f.source) && (f.repo === "all" || c.repo === f.repo)
    && (!q || [c.ident, c.title, c.repo, c.reason, c.pr ? `#${c.pr.number}` : ""].some((s) => s.toLowerCase().includes(q))));
};
export const chipCounts = (cards: Card[]) => Object.fromEntries(CHIPS.map(([k]) => [k, cards.filter((c) => chipMatch(c, k)).length])) as Record<Chip, number>;
export const reposOf = (cards: Card[]) => [...new Set(cards.map((c) => c.repo).filter(Boolean))].sort();

/** Stat cards. `working` is every live agent (board.working); the breakdown counts the board's Working cards by role. */
export const stats = (b: Board) => {
  const col = (k: Column) => b.cards.filter((c) => c.column === k);
  const need = col("needsYou"), work = col("working"), blocked = col("blocked");
  const role = (r: Role) => work.filter((c) => c.role === r).length;
  return {
    needsYou: need.length, plans: need.filter((c) => c.action === "approvePlan").length,
    working: b.working, max: b.max, coding: role("coder"), investigating: role("investigator"), reviewing: role("reviewer"),
    blocked: blocked.length, inferred: blocked.filter((c) => c.action === "dismissBlocker").length,
    open: b.cards.filter((c) => c.column !== "done").length,
  };
};

/** Linear priority 1 urgent … 4 low → P0 … P3; none → "". */
export const prioLabel = (p: number) => (p >= 1 && p <= 4 ? `P${p - 1}` : "");

export const ago = (iso: string, now = Date.now()) => {
  const t = Date.parse(iso);
  if (!iso || Number.isNaN(t)) return "";
  const m = Math.max(0, Math.round((now - t) / 60000));
  return m < 1 ? "now" : m < 60 ? `${m}m ago` : m < 1440 ? `${Math.round(m / 60)}h ago` : `${Math.round(m / 1440)}d ago`;
};

export const PIPELINE: [PvStage, string][] = [["triage", "Triage"], ["investigate", "Investigate"], ["plan", "Plan"], ["implement", "Implement"], ["review", "Review"], ["merged", "Merged"]];
export const stepOf = (s: PvStage | null) => (s ? PIPELINE.findIndex(([k]) => k === s) : -1);

export type Tone = "amber" | "red" | "green" | "blue" | "grey";
export const pill = (c: Card): { label: string; tone: Tone } =>
  c.column === "needsYou" ? { label: c.action === "approvePlan" ? "Plan ready" : "Needs you", tone: "amber" }
  : c.column === "done" ? { label: "Merged", tone: "green" }
  : failing(c) ? { label: "CI failing", tone: "red" }
  : c.column === "working" ? { label: "Agent running", tone: "green" }
  : c.column === "blocked" ? { label: "Blocked", tone: "red" }
  : c.column === "queued" ? { label: "Queued", tone: "grey" }
  : { label: "Watching", tone: "blue" };

export const BOARD: [string, Column[]][] = [["Needs you", ["needsYou"]], ["In flight", ["working"]], ["Queued / blocked", ["queued", "blocked"]], ["Watching", ["watching"]], ["Done", ["done"]]];
```

- [ ] **Step 4: Run the check and the build**

Run: `node scripts/pavilionView.check.ts && pnpm build`
Expected: `pavilionView ok`, then the build passes.

- [ ] **Step 5: Commit**

```bash
git add src/types.ts src/pavilionView.ts scripts/pavilionView.check.ts
git commit -m "Pavilion UI: view helpers for filters, counts and labels"
```

---

### Task 3: Inspector

**Files:**
- Create: `src/PavilionInspector.tsx`
- Modify: `src/Icons.tsx` (only if an icon is needed: small line icons with the existing `S` constant)
- Modify: `src/App.css` (append an `/* Pavilion inspector */` block)

**Interfaces:**
- Consumes: `Card`, `Session`, `LogEntry` (types.ts); `pill`, `PIPELINE`, `stepOf`, `ROLE_LABEL`, `VIEW_LABEL`, `SOURCE_LABEL`, `prioLabel` (pavilionView.ts); `SessionPane` (named export of `src/MeditationPane.tsx`, props `{ id, session, title?, noun? }`); `Markdown` (default export, prop `text`); `PrIcon`, `SEV`, `OVERALL`; `openUrl` from `@tauri-apps/plugin-opener`; commands `pavilion_approve { issueId, plan }`, `pavilion_dismiss_blocker { issueId }`, `pavilion_start_now { issueId }`.
- Produces: `export default function PavilionInspector({ card, sessions, log, run, onOpen, onClose }: { card: Card; sessions: Record<string, Session>; log: LogEntry[]; run: (cmd: string, args: Record<string, unknown>) => Promise<boolean>; onOpen: (view: string, id: string) => void; onClose?: () => void })`. `onClose` is only passed in drawer mode, where it shows a ✕.

- [ ] **Step 1: Build the component** following the mockup's inspector (see `docs/specs/2026-10-19-pavilion-ui-mockup-list.png`, right panel), top to bottom:
  1. **Header row:** a caps label "INSPECTOR" and a small chip with `SOURCE_LABEL[card.source]` minus the plural (or just the source), plus ✕ when `onClose` is set.
  2. **Ident line** (mono, small): `card.ident` (for issues), `PrIcon` + `#${pr.number}` when there is a PR, `card.repo`. Join with " · " only between present parts.
  3. **Title** (large, bold), then the status pill (`pill(card)`; tone → chip colour class), then `card.summary` (muted) when non-empty.
  4. **Tabs:** Overview, Plan (when `card.plan`), Terminal (when `card.session && sessions[card.session]`), Activity (when `log.length`). Use local tab state. When the selected card changes the component is re-keyed by the parent, so the tab resets.
  5. **Overview:**
     - **Pipeline:** when `card.stage` is set, a 6-step stepper from `PIPELINE`. Steps before the current one show done (✓, green tint), the current one is accent, later ones are dim. A right-aligned "N% through" uses `Math.round(stepOf(stage) / 5 * 100)`.
     - **Your decision:** when `card.column === "needsYou"`, an amber-tinted box showing `card.reason` and buttons by `card.action`:
       - `approvePlan`: **Approve plan** (primary; `pavilion_approve` with the current plan text, see Plan tab) and **Edit plan** (switches to the Plan tab).
       - `startNow`: **Investigate anyway** (`pavilion_start_now`).
       - No action (a question, a draft PR, a review draft): **Open in {VIEW_LABEL[card.view]}** (primary; `onOpen(card.view, card.target)`).
     - **Blocked:** when the action is `dismissBlocker` (blocked column), a box with the reason and **Dismiss blocker**.
     - **Queued:** when the column is queued, a muted box "Waiting for a free agent slot" and **Start now**.
     - **Judgement** (when `card.verdict`): `verdict.reason`, right-aligned `${Math.round(confidence*100)}% confidence`, and chips "Local model" or "Claude-checked" by `verdict.by`, plus "From local state".
     - **Linked:** rows with label left and mono value right. "Linear issue" (`card.ident`, clickable via `openUrl(card.url)`, only for issues), "Pull request" (`#n`, `openUrl(pr.url)`), "Repository", "Worker" (`ROLE_LABEL[role]`), "Runtime" "Claude Code".
     - **Checks** (when `card.pr`):
       - A heading with the CI state from `OVERALL[pr.overall]`.
       - One row per `pr.failingChecks` entry (red "Failing").
       - Rows for "Unresolved comments" (`pr.unresolvedThreads`) and "Merge conflict" (`pr.conflict`) when non-zero or true.
       - With no failing checks and CI passing, one green row "All checks passing".
  6. **Plan tab:** the existing plan editor behaviour. Keep a `text` state seeded from `card.plan`, Edit/Preview toggle, `Markdown` preview, textarea editor. The parent keys the inspector by `card.key + ":" + (card.plan ?? "")`, so a new plan resets edits but other board updates don't. **Approve plan** in Overview and in the footer must send this same edited `text`; lift the text state to the inspector root so both buttons see it.
  7. **Terminal tab:** `<SessionPane id={card.session} session={sessions[card.session]} title={`Investigate ${card.ident}`} noun="investigation" />`, filling the panel height.
  8. **Activity tab:** `log` entries, newest first. Show the time (`new Date(at)` formatted short) and the message.
  9. **Sticky footer:** a secondary button "Open in {VIEW_LABEL[card.view]}" (`onOpen`) and, when there is a primary action (Approve plan / Start now / Dismiss blocker), that action as the primary button.

  Buttons call `run(...)`, which reports errors through the parent's banner. Disable a mutating button while its call is in flight (local `busy` state) to prevent double submits.

- [ ] **Step 2: Styles.** Append `/* Pavilion inspector */` to `src/App.css`, with classes prefixed `pvi-`. Match the mockup's spacing and hierarchy:
  - Sections separated by `border-top: 1px solid var(--line)`, padding 16–20px.
  - Caps labels: 10–11px, letter-spacing .14em, colour `var(--dim)`.
  - Title 20px bold. The decision box is tinted with `color-mix(in srgb, var(--amber) 10%, var(--surface))` and has a matching border.
  - Stepper dots are 22px circles joined by 2px lines.
  - Sticky footer at the bottom with `background: var(--surface)`.
  - Theme variables only.

- [ ] **Step 3: Build**

Run: `pnpm build`
Expected: passes. The component isn't mounted yet; that happens in Task 4.

- [ ] **Step 4: Commit**

```bash
git add src/PavilionInspector.tsx src/App.css src/Icons.tsx
git commit -m "Pavilion UI: inspector with pipeline, decisions, judgement, links and checks"
```

---

### Task 4: Page: header, stats, queue (list and board), layout

**Files:**
- Rewrite: `src/Pavilion.tsx`
- Modify: `src/App.tsx` (pass `logs`)
- Modify: `src/App.css` (replace the old `/* Pavilion board */` block, the `.pavilion .pv-body` … `.pv-plan textarea` rules, with the new page styles)
- Modify: `src/Icons.tsx` (only if needed)

**Interfaces:**
- Consumes: Task 2 helpers, Task 3 `PavilionInspector`; commands `pavilion_set_paused { paused }`, `refresh_inbox`, `refresh_prs`, `refresh_reviews` (no args).
- Produces: `export default function Pavilion({ board, sessions, logs, onOpen }: { board: Board; sessions: Record<string, Session>; logs: Record<string, LogEntry[]>; onOpen: (view: string, id: string) => void })`. In `App.tsx` pass `logs={logs}`.

- [ ] **Step 1: Rewrite `src/Pavilion.tsx`.** Use `docs/specs/2026-10-19-pavilion-ui-mockup-list.png` and `docs/specs/2026-10-19-pavilion-ui-mockup-board.png`, the "Design decisions" wording table at the top of this plan, and `docs/specs/2026-10-19-pavilion-ui-mockup.html` for exact spacing.
  - **State:**
    - `filters: Filters` (start `NO_FILTERS`).
    - `mode: "list" | "board"` (from and to `localStorage["murim.pavilion.view"]`, default "list").
    - `sel: string | null`.
    - `err`.
    - `drawer` (narrow-mode inspector open).
    - `run(cmd, args)` as today, returning a boolean and setting the banner error.
  - **Selected card:** `board.cards.find(c => c.key === sel)`, falling back to the first card of `filterCards(...)`, else `board.cards[0]`, else none. Selection survives filtering: if the selected card is filtered out it stays selected while it still exists on the board.
  - **Header** (inside a scroll container, not the drag topbar):
    - Eyebrow "PAVILION", h1 "Pavilion" with an accent "." (`var(--brand)`), subhead.
    - Right side:
      - Status pills: `local model offline` (amber) when `board.localOffline`, `Linear error` (red, title = error) when `board.error`, and `updated {time}` when `board.updated`.
      - **Refresh**: invokes `refresh_inbox`, `refresh_prs`, `refresh_reviews`.
      - **Pause agents** / **Resume agents**: toggles `pavilion_set_paused`. Primary style when paused, so resuming stands out.
    - Keep a thin top drag region (`data-tauri-drag-region`) like other panes so the window can be dragged.
  - **Stats:** 4 cards from `stats(board)`, using the wording from the design table.
    - Big two-digit numbers (`String(n).padStart(2, "0")`).
    - Number colours: Needs you amber when > 0, Working green, Blocked `var(--text)`, Open `var(--text)`.
    - Clicking a card sets the chip: needsYou, working, blocked, or all for Open. The active chip's card gets the highlighted border.
  - **Queue panel** (rounded panel, as in the mockup):
    - Title row: "Work queue" or "Work by state", a count badge (`board.cards.length`) and the List/Board segmented toggle.
    - Chips: `CHIPS` with `chipCounts(board.cards)`.
    - Filter bar:
      - Search input (placeholder "Search issues, PRs, repos…", a `/` kbd hint). A window keydown listener focuses it on `/` when focus isn't in an input or textarea.
      - Source select: "All sources", then `SOURCE_LABEL`.
      - Repo select: "All repositories", then `reposOf(board.cards)`.
      - **Clear** text button (resets to `NO_FILTERS`, keeps mode).
    - Result line: `Showing {shown} of {board.cards.length}` and "Click a row to inspect".
    - **List mode:** one row per filtered card, matching the mockup row.
      - Ident line: ident (mono, brand colour), `PrIcon #n` when there's a PR, repo (muted), and a priority badge `prioLabel` in a small bordered box when non-empty. Separators appear only between present parts.
      - Bold title, then the reason line coloured by the pill tone, with a small icon.
      - Right column: the pill, `ROLE_LABEL[role]` with a one-letter avatar square, `ago(updatedAt)`, and a 50px progress bar filled to `(stepOf(stage)+1)/6` when a stage exists.
      - The selected row gets the brand-tinted border and background, plus a left accent bar.
      - Clicking selects the card and, in narrow mode, opens the drawer.
      - Rows are `role="button"` `tabIndex=0` with Enter/Space handling. No nested buttons in rows.
    - **Board mode:** `BOARD` columns, side by side with horizontal scroll, each with a header label and count. Mini cards show the ident line (ident + PR), title, reason (2-line clamp), and a footer of repo left and role right. Click selects. Empty columns show a dim "Nothing here".
    - **Empty states:**
      - No cards at all: the current empty-state text.
      - Filters match nothing: "Nothing matches these filters" and a Clear button.
  - **Inspector:** `<PavilionInspector key={card.key + ":" + (card.plan ?? "")} card={card} sessions={sessions} log={logs[card.key] ?? []} run={run} onOpen={onOpen} />` in a right column (about 380px), inside the same rounded panel as the queue, as in the mockup.
    - Under 1100px window width (CSS media query plus a `matchMedia` listener), the inspector is hidden until a row is clicked. It then opens as an overlay drawer from the right with a dimmed backdrop, and `onClose` closes it.
  - **Errors:** the error banner shows under the header.
- [ ] **Step 2: Styles.** Replace the old Pavilion block in `src/App.css` with `/* Pavilion */` page styles, prefixed `pv-`.
  - Page padding 28px 32px.
  - Eyebrow 11px caps in brand colour with letter-spacing; h1 34px, weight 800, letter-spacing -.03em.
  - Stat grid: `repeat(4, 1fr)` with a 12px gap, 2 columns under 900px. Cards are radius 12, `var(--surface)` with a `var(--line)` border, and the label row has an icon.
  - The queue panel and inspector share one rounded container with the inspector separated by `border-left`.
  - Chips are pill buttons with counts.
  - Rows are separated by `var(--line)`.
  - The pill tones use the `.chip red|amber|green|blue` colours (reuse the existing `.chip` classes where you can).
  - Theme variables only.
  - Delete the now-unused old `.pv-*` rules.
- [ ] **Step 3: App wiring.** In `src/App.tsx` change the Pavilion element to pass `logs={logs}`.
- [ ] **Step 4: Verify**

Run: `node scripts/pavilionView.check.ts && pnpm build && cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/Pavilion.tsx src/App.tsx src/App.css src/Icons.tsx
git commit -m "Pavilion UI: header, stats, filterable list and board, inspector layout"
```
