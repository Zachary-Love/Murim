import { invoke } from "@tauri-apps/api/core";

export type Item = {
  issue: { id: string; identifier: string; title: string; url: string; stateName: string; stateType: string; priority: number; labels: string[] };
  repo: { path: string; source: string } | null;
  resolving: boolean;
};
export type Repo = { name: string; path: string };

const rank = (i: Item) => (i.issue.stateType === "started" ? 0 : 1) * 10 + (i.issue.priority || 9);
export const sortItems = (a: Item[]) => [...a].sort((x, y) => rank(x) - rank(y));
export const repoName = (i: Item) => i.repo?.path.split("/").filter(Boolean).pop() ?? "";
const PRIORITY = ["", "Urgent", "High", "Medium", "Low"];

export default function Inbox({ items, repos }: { items: Item[]; repos: Repo[] }) {
  if (!items.length) return <p className="empty">No tasks</p>;
  return (
    <ul className="tasks">
      {sortItems(items).map((i) => (
        <li key={i.issue.id} className="task">
          <div className="task-main">
            <span className="ident">{i.issue.identifier}</span> {i.issue.title}
            <div className="meta">
              {i.issue.stateName}
              {PRIORITY[i.issue.priority] && ` · ${PRIORITY[i.issue.priority]}`}
              {i.issue.labels.map((l) => <span key={l} className="tag">{l}</span>)}
            </div>
          </div>
          {i.resolving ? <span className="badge">matching…</span> : i.repo && <span className="badge">{i.repo.source}</span>}
          <select value={i.repo?.path ?? ""} onChange={(e) => invoke("set_issue_repo", { issueId: i.issue.id, path: e.target.value })}>
            {!i.repo && <option value="" disabled>Pick repo…</option>}
            {repos.map((r) => <option key={r.path} value={r.path}>{r.name}</option>)}
          </select>
          <button disabled title="Phase 3">Start</button>
        </li>
      ))}
    </ul>
  );
}
