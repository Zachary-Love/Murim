import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

export type Item = {
  issue: { id: string; identifier: string; title: string; url: string; stateName: string; stateType: string; priority: number; labels: string[] };
  repo: { path: string; source: string } | null;
  resolving: boolean;
};
export type Mode = "normal" | "allowlist" | "skip";
export type Status = "starting" | "working" | "needsInput" | "idle" | "ended";
export type Session = { issueId: string; worktree: string; sessionId: string | null; status: Status; questions: unknown; ptyAlive: boolean; permissionMode: Mode; tempering?: boolean; replyMode?: ReplyMode; escalations?: Escalation[]; prFlow?: string };
export type ReplyMode = "off" | "terse" | "full";
export type Escalation = { id: string; threadId: string; path: string; comment: string; reason: string };
export type LogEntry = { issueId: string; at: number | string; message: string };
export const LABEL: Record<Status, string> = { starting: "Starting", working: "Working", needsInput: "Needs input", idle: "Idle", ended: "Ended" };
export const STEP: Record<string, string> = { none: "Implementing", reviewing: "Reviewing", opened: "PR draft", ready: "Ready for review" };
export const step = (s: Session) => <span className="step">{STEP[s.prFlow ?? "none"] ?? "Implementing"}</span>;
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
export const prBadge = (p: PrHealth) => (
  <a className={`badge pr ${SEV[p.overall] ?? "grey"}`} title={p.title} onClick={() => openUrl(p.url)}>#{p.number} {OVERALL[p.overall] ?? p.overall}</a>
);

const rank = (i: Item) => (i.issue.stateType === "started" ? 0 : 1) * 10 + (i.issue.priority || 9);
const live = (s?: Session) => s?.status === "starting" || s?.status === "working";
const sess = (m: Record<string, Session>, i: Item) => m[i.issue.id];
export const sortItems = (a: Item[], m: Record<string, Session>, prs: PrMap = {}) =>
  [...a].sort((x, y) => {
    const g = (i: Item) => {
      const sv = SEV[prs[i.issue.id]?.overall];
      return sess(m, i)?.status === "needsInput" || sess(m, i)?.escalations?.length ? 0 : sv === "red" ? 1 : sv === "amber" ? 2 : sv === "blue" ? 3 : live(sess(m, i)) ? 4 : 5;
    };
    return g(x) - g(y) || rank(x) - rank(y);
  });
export const repoName = (i: Item) => i.repo?.path.split("/").filter(Boolean).pop() ?? "";
const PRIORITY = ["", "Urgent", "High", "Medium", "Low"];

type Props = { items: Item[]; repos: Repo[]; sessions: Record<string, Session>; prs: PrMap; defaultMode: Mode; onOpen: (id: string) => void };

export default function Inbox({ items, repos, sessions, prs, defaultMode, onOpen }: Props) {
  const [modes, setModes] = useState<Record<string, Mode>>({});
  const [errs, setErrs] = useState<Record<string, string>>({});
  const start = (id: string, mode: Mode) =>
    invoke("start_task", { issueId: id, permissionMode: mode })
      .then(() => { setErrs((e) => ({ ...e, [id]: "" })); onOpen(id); })
      .catch((e) => setErrs((x) => ({ ...x, [id]: String(e) })));
  if (!items.length) return <p className="empty">No tasks</p>;
  return (
    <ul className="tasks">
      {sortItems(items, sessions, prs).map((i) => {
        const se = sess(sessions, i);
        const mode = modes[i.issue.id] ?? defaultMode;
        return (
        <li key={i.issue.id} className="task">
          <div className="task-main">
            <a className="ident" onClick={() => openUrl(i.issue.url)}>{i.issue.identifier}</a> {i.issue.title}
            <div className="meta">
              {i.issue.stateName}
              {PRIORITY[i.issue.priority] && ` · ${PRIORITY[i.issue.priority]}`}
              {i.issue.labels.map((l) => <span key={l} className="tag">{l}</span>)}
            </div>
            {errs[i.issue.id] && <div className="err">{errs[i.issue.id]}</div>}
          </div>
          <span className={`badge ${se?.status ?? ""}`}>{se ? LABEL[se.status] : "Todo"}</span>
          {se && step(se)}
          {prs[i.issue.id] && prBadge(prs[i.issue.id])}
          {!!se?.escalations?.length && <span className="badge red" title="Escalations">{se.escalations.length}</span>}
          {prs[i.issue.id] && (
            <button className={se?.tempering ? "on" : ""} title="Tempering" onClick={() =>
              invoke("set_tempering", { issueId: i.issue.id, on: !se?.tempering })
                .catch((e) => setErrs((x) => ({ ...x, [i.issue.id]: String(e) })))}>🔥 Temper</button>
          )}
          {i.resolving ? <span className="badge">matching…</span> : i.repo && <span className="badge">{i.repo.source}</span>}
          <select value={i.repo?.path ?? ""} onChange={(e) => invoke("set_issue_repo", { issueId: i.issue.id, path: e.target.value })}>
            {!i.repo && <option value="" disabled>Pick repo…</option>}
            {repos.map((r) => <option key={r.path} value={r.path}>{r.name}</option>)}
          </select>
          <select value={mode} title="Permission mode" onChange={(e) => setModes({ ...modes, [i.issue.id]: e.target.value as Mode })}>
            <option value="normal">normal</option>
            <option value="allowlist">allowlist</option>
            <option value="skip">skip</option>
          </select>
          {mode === "skip" && <span title="Skips all permission prompts">⚠</span>}
          <button disabled={!i.repo || !!se?.ptyAlive} onClick={() => start(i.issue.id, mode)}>Start</button>
          {se && <button onClick={() => onOpen(i.issue.id)}>Open</button>}
        </li>
        );
      })}
    </ul>
  );
}
