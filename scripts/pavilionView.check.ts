import assert from "node:assert/strict";
import { filterCards, chipCounts, reposOf, stats, prioLabel, ago, stepOf, pill, NO_FILTERS, agentStatus, PICK, pickTitle } from "../src/pavilionView.ts";

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

// robot pill + header status
const b = { cards: [], working: 0, max: 3, paused: false, localOffline: false, updated: 0, error: null, model: "llama3.2", thinking: null, judging: 0, nextRun: 1000 } as any;
assert.equal(agentStatus(b, 958).label, "llama3.2 idle · next check 0:42");
assert.equal(agentStatus({ ...b, nextRun: 0 }, 958).label, "llama3.2 idle · ");
assert.equal(agentStatus(b, 2000).label, "llama3.2 idle · next check 0:00"); // never negative
assert.equal(agentStatus({ ...b, thinking: "MUR-3", judging: 3, paused: true }, 958).label, "llama3.2 thinking · MUR-3 · 2 waiting"); // thinking wins over paused
assert.equal(agentStatus({ ...b, paused: true }, 900).tone, "amber");
assert.equal(PICK.queued.label, "In queue");
assert.match(pickTitle(card({ pick: "queued", verdict: { by: "local", confidence: 0.82, reason: "clear spec" } })), /Local model · 82% sure: clear spec/);
assert.equal(pickTitle(card({ pick: "thinking" })), "");
