export type Item = {
  issue: { id: string; identifier: string; title: string; url: string; stateName: string; stateType: string; priority: number; labels: string[] };
  repo: { path: string; source: string } | null;
  resolving: boolean;
};
export type Mode = "normal" | "allowlist" | "skip";
// Claude Code's own names for what each mode passes to `claude`.
export const MODE_LABEL: Record<Mode, string> = { normal: "Ask permissions", allowlist: "Accept edits + dev tools", skip: "Bypass permissions" };
export type Status = "starting" | "working" | "needsInput" | "idle" | "ended";
export type Session = { issueId: string; worktree: string; sessionId: string | null; status: Status; questions: unknown; ptyAlive: boolean; permissionMode: Mode; tempering?: boolean; replyMode?: ReplyMode; escalations?: Escalation[]; prFlow?: string };
export type ReplyMode = "off" | "terse" | "full";
export type Escalation = { id: string; threadId: string; path: string; comment: string; reason: string };
export type LogEntry = { issueId: string; at: number | string; message: string };
export const LABEL: Record<Status, string> = { starting: "Starting", working: "Working", needsInput: "Needs input", idle: "Idle", ended: "Ended" };
export const STEP: Record<string, string> = { none: "Implementing", reviewing: "Quality pass", awaitingReview: "Draft PR · your review", opened: "Draft PR · your review", ready: "Ready for review" };
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

// The task's overall stage, from Linear state + Claude session + PR. Drives the sidebar status dot.
export type Stage = "todo" | "progress" | "needsReview" | "codeReview" | "done";
export const STAGE_LABEL: Record<Stage, string> = { todo: "Todo", progress: "In progress", needsReview: "Needs your review", codeReview: "In code review", done: "Done" };
export const stage = (i: Item, s?: Session, pr?: PrHealth): Stage => {
  if (i.issue.stateType === "completed" || pr?.overall === "merged") return "done";
  if (pr && pr.state === "OPEN" && !pr.isDraft) return "codeReview";
  if (s?.status === "needsInput" || s?.prFlow === "awaitingReview" || s?.prFlow === "opened" || s?.escalations?.length || (pr?.isDraft && s?.status !== "working" && s?.status !== "starting")) return "needsReview";
  if (i.issue.stateType === "started" || s) return "progress";
  return "todo";
};
