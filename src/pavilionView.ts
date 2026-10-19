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
