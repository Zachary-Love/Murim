import { useState, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import IssueView from "./IssueView";
import { Item, Session, PrHealth, Repo, Mode, LABEL, OVERALL, SEV, STEP, LogEntry } from "./types";

const prBadge = (p: PrHealth) => (
  <a className={`badge pr ${SEV[p.overall] ?? "grey"}`} title={p.title} onClick={() => openUrl(p.url)}>#{p.number} {OVERALL[p.overall] ?? p.overall}</a>
);
type Tab = "issue" | "claude" | "pr";

// ponytail: questions shape is backend-defined JSON; accept {questions:[{question}]}, [{question}], or strings.
const texts = (q: unknown): string[] => {
  const a = Array.isArray(q) ? q : (q as { questions?: unknown[] })?.questions;
  return (Array.isArray(a) ? a : []).map((x) => (typeof x === "string" ? x : (x as { question?: string })?.question ?? JSON.stringify(x)));
};

export default function TaskPane({ item, session, pr, log, repos, defaultMode }: { item: Item; session?: Session; pr?: PrHealth; log: LogEntry[]; repos: Repo[]; defaultMode: Mode }) {
  const [tab, setTab] = useState<Tab>(session?.status === "needsInput" ? "claude" : "issue");
  const [mode, setMode] = useState<Mode>(defaultMode);
  const [prompt, setPrompt] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [own, setOwn] = useState<string | null>(null); // escalation id with open textarea
  const [text, setText] = useState("");
  const id = item.issue.id;
  const run = (cmd: string, args: object = {}) => invoke(cmd, { issueId: id, ...args }).then(() => setErr("")).catch((e) => setErr(String(e)));
  const resolve = async (escalationId: string, choice: string, txt: string | null = null) => {
    setBusy(true);
    await run("resolve_escalation", { escalationId, choice, text: txt });
    setBusy(false); setOwn(null); setText("");
  };
  const ago = (at: number | string) => {
    const s = Math.max(0, Math.round((Date.now() - (typeof at === "number" ? (at < 1e12 ? at * 1000 : at) : Date.parse(at))) / 1000));
    return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : `${Math.floor(s / 3600)}h ago`;
  };
  const qs = session?.status === "needsInput" ? texts(session.questions) : [];
  const alive = !!session?.ptyAlive;
  const start = () => run("start_task", { permissionMode: mode }).then(() => setTab("claude"));
  const pickRepo = !item.repo || item.repo.source === "semantic" || item.repo.source === "claude";
  const T = ({ id, children }: { id: Tab; children: ReactNode }) => <button className={`tab ${tab === id ? "on" : ""}`} onClick={() => setTab(id)}>{children}</button>;
  return (
    <div className="pane">
      <div className="pane-head">
        <a className="ident" onClick={() => openUrl(item.issue.url)}>{item.issue.identifier}</a>
        <span className="pane-title" title={item.issue.title}>{item.issue.title}</span>
        {session && <span className={`badge ${session.status}`}>{LABEL[session.status]}</span>}
        {session && <span className="step">{STEP[session.prFlow ?? "none"] ?? "Implementing"}</span>}
        {pr && prBadge(pr)}
      </div>
      <div className="pane-head actions">
        {item.resolving && <span className="badge">matching…</span>}
        {pickRepo && (
          <select value={item.repo?.path ?? ""} onChange={(e) => invoke("set_issue_repo", { issueId: id, path: e.target.value }).catch((x) => setErr(String(x)))}>
            {!item.repo && <option value="" disabled>Pick repo…</option>}
            {repos.map((r) => <option key={r.path} value={r.path}>{r.name}</option>)}
          </select>
        )}
        {!alive && (
          <>
            <select value={mode} title="Permission mode" onChange={(e) => setMode(e.target.value as Mode)}>
              <option value="normal">normal</option>
              <option value="allowlist">allowlist</option>
              <option value="skip">skip</option>
            </select>
            {mode === "skip" && <span title="Skips all permission prompts">⚠</span>}
            <button disabled={!item.repo} onClick={start}>Start</button>
          </>
        )}
        {session && !alive && (
          <>
            <input placeholder="Prompt (optional)" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
            <button onClick={() => run("resume_task", { prompt: prompt || null }).then(() => setTab("claude"))}>Resume</button>
          </>
        )}
        {alive && <button onClick={() => run("stop_task")}>Stop</button>}
        {pr && session && (
          <>
            <button className={session.tempering ? "on" : ""} onClick={() => run("set_tempering", { on: !session.tempering })}>🔥 Temper</button>
            <select value={session.replyMode ?? "terse"} title="Reply mode" onChange={(e) => run("set_reply_mode", { mode: e.target.value })}>
              <option value="off">Replies: off</option>
              <option value="terse">Replies: terse</option>
              <option value="full">Replies: full</option>
            </select>
          </>
        )}
        <button onClick={() => openUrl(item.issue.url)}>Open in Linear</button>
      </div>
      {err && <div className="banner">{err}</div>}
      <div className="tabs"><T id="issue">Issue</T><T id="claude">Claude</T><T id="pr">PR{!!session?.escalations?.length && ` (${session.escalations.length})`}</T></div>
      {tab === "issue" && <IssueView issueId={id} />}
      {tab === "claude" && (
        <>
          {qs.map((q, n) => <div key={n} className="question">{q}</div>)}
          <Terminal issueId={id} />
        </>
      )}
      {tab === "pr" && (
        <div className="prtab">
          {!pr && <p className="empty">No pull request</p>}
      {pr && (
        <div className={`prbox ${SEV[pr.overall] ?? "grey"}`}>
          <div>{prBadge(pr)} <a onClick={() => openUrl(pr.url)}>{pr.title}</a> <button onClick={() => invoke("refresh_prs").catch((e) => setErr(String(e)))}>↻ PR</button></div>
          <div className="meta">
            CI: {pr.ci}{pr.failingChecks.length > 0 && ` (${pr.failingChecks.join(", ")})`}
            {" · "}{pr.unresolvedThreads} unresolved thread{pr.unresolvedThreads === 1 ? "" : "s"}
            {" · "}review: {pr.reviewDecision ?? (pr.changesRequested ? "CHANGES_REQUESTED" : "none")}
            {pr.conflict && <b className="conflict"> · merge conflict</b>}
          </div>
        </div>
      )}
      {pr && session && (
        <div className="prbox temper">
          {(session.escalations ?? []).map((x) => (
            <div key={x.id} className="esc">
              <div className="meta">{x.path}</div>
              <blockquote>{x.comment}</blockquote>
              <div>Claude: {x.reason}</div>
              <button disabled={busy} onClick={() => resolve(x.id, "doIt")}>Do it anyway</button>
              <button disabled={busy} onClick={() => resolve(x.id, "postPushback")}>Post Claude's pushback</button>
              <button disabled={busy} onClick={() => setOwn(own === x.id ? null : x.id)}>Write my own reply</button>
              <button disabled={busy} onClick={() => resolve(x.id, "ignore")}>Ignore</button>
              {own === x.id && (
                <div>
                  <textarea value={text} onChange={(e) => setText(e.target.value)} />
                  <button disabled={busy || !text.trim()} onClick={() => resolve(x.id, "ownReply", text)}>Send</button>
                </div>
              )}
            </div>
          ))}
          {log.length > 0 && <ul className="tlog">{log.map((l, n) => <li key={n}><span className="meta">{ago(l.at)}</span> {l.message}</li>)}</ul>}
        </div>
      )}
        </div>
      )}
    </div>
  );
}
