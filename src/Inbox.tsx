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
export type Session = { issueId: string; worktree: string; sessionId: string | null; status: Status; questions: unknown; ptyAlive: boolean; permissionMode: Mode };
export const LABEL: Record<Status, string> = { starting: "Starting", working: "Working", needsInput: "Needs input", idle: "Idle", ended: "Ended" };
export type Repo = { name: string; path: string };

const rank = (i: Item) => (i.issue.stateType === "started" ? 0 : 1) * 10 + (i.issue.priority || 9);
const live = (s?: Session) => s?.status === "starting" || s?.status === "working";
const sess = (m: Record<string, Session>, i: Item) => m[i.issue.id];
export const sortItems = (a: Item[], m: Record<string, Session>) =>
  [...a].sort((x, y) => {
    const g = (i: Item) => (sess(m, i)?.status === "needsInput" ? 0 : live(sess(m, i)) ? 1 : 2);
    return g(x) - g(y) || rank(x) - rank(y);
  });
export const repoName = (i: Item) => i.repo?.path.split("/").filter(Boolean).pop() ?? "";
const PRIORITY = ["", "Urgent", "High", "Medium", "Low"];

type Props = { items: Item[]; repos: Repo[]; sessions: Record<string, Session>; defaultMode: Mode; onOpen: (id: string) => void };

export default function Inbox({ items, repos, sessions, defaultMode, onOpen }: Props) {
  const [modes, setModes] = useState<Record<string, Mode>>({});
  const [errs, setErrs] = useState<Record<string, string>>({});
  const start = (id: string, mode: Mode) =>
    invoke("start_task", { issueId: id, permissionMode: mode })
      .then(() => { setErrs((e) => ({ ...e, [id]: "" })); onOpen(id); })
      .catch((e) => setErrs((x) => ({ ...x, [id]: String(e) })));
  if (!items.length) return <p className="empty">No tasks</p>;
  return (
    <ul className="tasks">
      {sortItems(items, sessions).map((i) => {
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
