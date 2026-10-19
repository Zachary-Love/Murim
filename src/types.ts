export type Item = {
  issue: { id: string; identifier: string; title: string; url: string; stateName: string; stateType: string; priority: number; labels: string[] };
  repo: { path: string; source: string } | null;
  resolving: boolean;
};
export type Mode = "normal" | "allowlist" | "skip";
export type AgentKind = "claude" | "custom";
// Claude Code's own names for what each mode passes to `claude`.
export const MODE_LABEL: Record<Mode, string> = { normal: "Ask permissions", allowlist: "Accept edits + dev tools", skip: "Bypass permissions" };
export type Status = "starting" | "working" | "needsInput" | "idle" | "ended";
export type Session = { issueId: string; worktree: string; sessionId: string | null; status: Status; questions: unknown; ptyAlive: boolean; permissionMode: Mode; tempering?: boolean; escalations?: Escalation[]; prFlow?: string; agent?: AgentKind; name?: string };
// Meditations: free-form sessions keyed "meditation:<hex>" + markdown artifacts (backend meditations.rs)
export type Artifact = { path: string; title: string; modified: number };
export type MedSel = { kind: "session" | "artifact"; id: string };
export const isMed = (id: string) => id.startsWith("meditation:");
export type ReplyMode = "off" | "terse" | "full";
export type Escalation = { id: string; threadId: string; path: string; comment: string; reason: string; ask?: boolean };
export type LogEntry = { issueId: string; at: number | string; message: string };
export type Repo = { name: string; path: string };
export type PrHealth = {
  number: number; url: string; title: string; state: string; isDraft: boolean; ci: string; failingChecks: string[];
  unresolvedThreads: number; changesRequested: boolean; conflict: boolean; reviewDecision?: string | null; overall: string;
};
export type PrMap = Record<string, PrHealth>;
export const SEV: Record<string, string> = {
  ciFailing: "red", changesRequested: "red", conflict: "red",
  unresolvedComments: "amber", ciPending: "amber", draft: "amber",
  awaitingReview: "blue", readyToMerge: "green", merged: "grey", closed: "grey",
};
export const OVERALL: Record<string, string> = {
  ciFailing: "CI failing", changesRequested: "Changes requested", conflict: "Conflict", unresolvedComments: "Comments",
  ciPending: "CI pending", draft: "Draft", awaitingReview: "Awaiting review", readyToMerge: "Ready to merge", merged: "Merged", closed: "Closed",
};
const rank = (i: Item) => (i.issue.stateType === "started" ? 0 : 1) * 10 + (i.issue.priority || 9);
const live = (s?: Session) => s?.status === "starting" || s?.status === "working";
export const sess = (m: Record<string, Session>, i: Item) => m[i.issue.id];
export const sortItems = (a: Item[], m: Record<string, Session>, prs: PrMap = {}) =>
  [...a].sort((x, y) => {
    const g = (i: Item) => {
      const sv = SEV[prs[i.issue.id]?.overall];
      return sess(m, i)?.status === "needsInput" || sess(m, i)?.escalations?.length ? 0 : sv === "red" ? 1 : sv === "amber" ? 2 : sv === "blue" ? 3 : live(sess(m, i)) ? 4 : 5;
    };
    return g(x) - g(y) || rank(x) - rank(y);
  });
export const repoName = (i: Item) => i.repo?.path.split("/").filter(Boolean).pop() ?? "";

// The task's overall stage, from Linear state + agent session + PR. Drives the sidebar status dot.
export type Stage = "todo" | "progress" | "needsReview" | "codeReview" | "done";
export const STAGE_LABEL: Record<Stage, string> = { todo: "Todo", progress: "In progress", needsReview: "Needs your review", codeReview: "In code review", done: "Done" };
export const stage = (i: Item, s?: Session, pr?: PrHealth): Stage => {
  if (i.issue.stateType === "completed" || pr?.overall === "merged") return "done";
  if (pr && pr.state === "OPEN" && !pr.isDraft) return "codeReview";
  if (s?.status === "needsInput" || s?.prFlow === "awaitingReview" || s?.prFlow === "opened" || s?.escalations?.length || (pr?.isDraft && s?.status !== "working" && s?.status !== "starting")) return "needsReview";
  if (i.issue.stateType === "started" || s) return "progress";
  return "todo";
};

// Review mode: other people's PRs (backend reviews.rs)
export type Bucket = "needsReview" | "draft" | "ciFailing" | "changesRequested" | "approved";
export type ReviewAuthor = { kind: "linear"; id: string; name: string; avatarUrl: string | null } | { kind: "unlinked"; login: string };
export type ReviewPr = {
  key: string; sessionId: string; owner: string; repo: string; number: number; base: string; body: string; branch: string;
  authorLogin: string; author: ReviewAuthor; requested: boolean; issue: { identifier: string; url: string } | null; bucket: Bucket; health: PrHealth;
};
export type ReviewFilters = { authors: string[]; statuses: Bucket[]; repos: string[] /* "owner/repo" */ };
export const BUCKET_LABEL: Record<Bucket, string> = { needsReview: "Needs review", draft: "Draft", ciFailing: "CI failing", changesRequested: "Changes requested", approved: "Approved" };
export type Draft = { summary: string; event: "COMMENT" | "APPROVE" | "REQUEST_CHANGES"; comments: { path: string; line: number; body: string }[] };

// Automations: scheduled runs (backend automations.rs). A run is a session keyed "automation:<auto-id>:<run-id>".
export type Schedule = { kind: "daily" | "weekdays" | "weekly" | "biweekly" | "monthly"; time: string; days: number[]; start: string; dayOfMonth: number };
export type AutoRun = { id: string; started: number; status: "started" | "failed" | "missed" | "skipped"; error?: string | null; sessionId: string | null };
export type Automation = { id: string; name: string; prompt: string; folder: string; schedule: Schedule; enabled: boolean; nextDue: number | null; runs: AutoRun[] };
export type AutoSel = { kind: "automation" | "run"; id: string }; // run: id = its sessionId
export const isAuto = (id: string) => id.startsWith("automation:");
export type RunStatus = "running" | "waiting" | "done" | "failed" | "missed" | "skipped";
export const runStatus = (r: AutoRun, m: Record<string, Session>): RunStatus => {
  if (r.status !== "started") return r.status;
  const s = r.sessionId ? m[r.sessionId] : undefined;
  return s?.status === "needsInput" ? "waiting" : live(s) ? "running" : "done";
};
export const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const hm = (t: string) => t.replace(/^0(\d)/, "$1");
export const scheduleText = (s: Schedule) => {
  const d = [...s.days].sort().map((i) => DAYS[i]).join(", ");
  return s.kind === "daily" ? `Daily ${hm(s.time)}` : s.kind === "weekdays" ? `Weekdays ${hm(s.time)}`
    : s.kind === "monthly" ? `Monthly · day ${s.dayOfMonth} ${hm(s.time)}`
    : `${s.kind === "weekly" ? "Weekly" : "Every 2 weeks"} · ${d} ${hm(s.time)}`;
};
export const until = (secs: number | null) => {
  if (secs == null) return "—";
  const s = secs - Date.now() / 1000;
  return s < 60 ? "now" : s < 3600 ? `in ${Math.floor(s / 60)}m` : s < 86400 ? `in ${Math.floor(s / 3600)}h` : `in ${Math.floor(s / 86400)}d`;
};

// Pavilion: the central board (backend pavilion.rs). Card keys: issue id, review PR key, or session id.
export type Column = "needsYou" | "working" | "blocked" | "queued" | "watching" | "done";
export type PvStage = "triage" | "investigate" | "plan" | "implement" | "review" | "merged";
export type Role = "investigator" | "coder" | "reviewer" | "automation" | "meditation";
export type Verdict = { answer: string; reason: string; confidence: number; by: "local" | "claude"; fingerprint: string };
export type Card = {
  key: string; column: Column; ident: string; title: string; repo: string; priority: number; reason: string;
  by: "local" | "claude" | null; action: "approvePlan" | "dismissBlocker" | "startNow" | null;
  view: string; target: string; pr: PrHealth | null; session: string | null; plan: string | null; updatedAt: string;
  source: "issue" | "review" | "agent"; stage: PvStage | null; role: Role | null; summary: string; url: string; verdict: Verdict | null;
};
export type Board = { cards: Card[]; working: number; max: number; paused: boolean; localOffline: boolean; updated: number; error: string | null };
