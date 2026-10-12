import { useState, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import IssueView from "./IssueView";
import ReviewView from "./ReviewView";
import { Item, Session, PrHealth, Repo, Mode, LABEL, OVERALL, SEV, STEP, LogEntry, MODE_LABEL } from "./types";
import { ClaudeMark, ClaudeIcon, PrIcon } from "./Icons";

const prBadge = (p: PrHealth) => (
  <button className={`chip ${SEV[p.overall] ?? "grey"}`} title={`Open PR: ${p.title}`} onClick={() => openUrl(p.url)}><PrIcon color="" title="" />#{p.number} {OVERALL[p.overall] ?? p.overall} ↗</button>
);
type Tab = "issue" | "claude" | "review" | "pr";

// ponytail: questions shape is backend-defined JSON; accept {questions:[{question}]}, [{question}], or strings.
const texts = (q: unknown): string[] => {
  const a = Array.isArray(q) ? q : (q as { questions?: unknown[] })?.questions;
  return (Array.isArray(a) ? a : []).map((x) => (typeof x === "string" ? x : (x as { question?: string })?.question ?? JSON.stringify(x)));
};

export default function TaskPane({ item, session, pr, log, repos, defaultMode }: { item: Item; session?: Session; pr?: PrHealth; log: LogEntry[]; repos: Repo[]; defaultMode: Mode }) {
  const [tab, setTab] = useState<Tab>(session?.status === "needsInput" ? "claude" : (session?.prFlow === "awaitingReview" || session?.prFlow === "opened") ? "review" : "issue");
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
  const resume = () => run("resume_task", { prompt: prompt || null }).then(() => setTab("claude"));
  const pickRepo = !item.repo || item.repo.source === "semantic" || item.repo.source === "claude";
  const T = ({ id, children }: { id: Tab; children: ReactNode }) => <button className={tab === id ? "on" : ""} onClick={() => setTab(id)}>{children}</button>;
  return (
    <div className="pane">
      <header className="pane-head">
        <button className="chip mono" title="Open in Linear" onClick={() => openUrl(item.issue.url)}>{item.issue.identifier} ↗</button>
        <h2 className="pane-title" title={item.issue.title}>{item.issue.title}</h2>
        {session && <span className={`chip status ${session.status}`}><ClaudeIcon status={session.status} />{LABEL[session.status]}</span>}
        {session && <span className="chip step">{STEP[session.prFlow ?? "none"] ?? "Implementing"}</span>}
        {pr && prBadge(pr)}
      </header>
      <div className="toolbar">
        {item.resolving && <span className="chip">Matching repo…</span>}
        {pickRepo && (
          <label className="field">Repo
            <select value={item.repo?.path ?? ""} onChange={(e) => invoke("set_issue_repo", { issueId: id, path: e.target.value }).catch((x) => setErr(String(x)))}>
              {!item.repo && <option value="" disabled>Pick repo…</option>}
              {repos.map((r) => <option key={r.path} value={r.path}>{r.name}</option>)}
            </select>
          </label>
        )}
        <div className="group claude">
          {alive ? (
            <button className="btn danger" onClick={() => run("stop_task")}>■ Stop Claude</button>
          ) : (
            <>
              <select value={mode} title="Claude Code permission mode" className={mode === "skip" ? "warn" : ""} onChange={(e) => setMode(e.target.value as Mode)}>
                {(Object.keys(MODE_LABEL) as Mode[]).map((m) => <option key={m} value={m}>{MODE_LABEL[m]}</option>)}
              </select>
              <button className={`btn ${session ? "" : "primary"}`} disabled={!item.repo} title={item.repo ? (session ? "Start a fresh Claude conversation in this worktree" : "") : "Pick a repo first"} onClick={start}><ClaudeMark />{session ? "New session" : "Start Claude"}</button>
            </>
          )}
        </div>
        {session && !alive && (
          <div className="group">
            <input placeholder="Follow-up prompt (optional)" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
            <button className={`btn ${session.prFlow === "awaitingReview" ? "" : "primary"}`} onClick={resume}>↻ Resume Claude</button>
          </div>
        )}
        {pr && session && (
          <div className="group">
            <button className={`btn toggle ${session.tempering ? "on" : ""}`} title="Babysit the PR: rerun CI, fix failures, answer reviews" onClick={() => run("set_tempering", { on: !session.tempering })}>🔥 Tempering {session.tempering ? "on" : "off"}</button>
            <select value={session.replyMode ?? "terse"} title="How Claude replies to review comments" onChange={(e) => run("set_reply_mode", { mode: e.target.value })}>
              <option value="off">Replies: off</option>
              <option value="terse">Replies: terse</option>
              <option value="full">Replies: full</option>
            </select>
          </div>
        )}
      </div>
      {err && <div className="banner">{err}</div>}
      <nav className="seg">
        <T id="issue">Issue</T>
        <T id="claude"><ClaudeMark size={12} />Claude<ClaudeIcon status={session?.status} /></T>
        {session && <T id="review">Review{(session.prFlow === "awaitingReview" || session.prFlow === "opened") && <span className="count attn">1</span>}</T>}
        <T id="pr">PR{pr && ` #${pr.number}`}{!!session?.escalations?.length && <span className="count">{session.escalations.length}</span>}</T>
      </nav>
      {tab === "issue" && <IssueView issueId={id} />}
      {tab === "review" && session && <ReviewView issueId={id} refreshKey={`${session.prFlow}:${session.status}`} onRequested={() => setTab("claude")} />}
      {tab === "claude" && (
        <>
          {qs.length > 0 && (
            <div className="question">
              <div className="q-head"><ClaudeMark />Claude is asking — answer in the terminal below</div>
              {qs.map((q, n) => <div key={n}>{q}</div>)}
            </div>
          )}
          {alive ? <Terminal issueId={id} /> : (
            // ponytail: no PTY means nothing to type into; show how to launch instead of a dead xterm.
            <div className="empty-state">
              <ClaudeMark size={28} />
              <p>{session ? "Claude isn't running for this task." : "Claude hasn't been started for this task."}</p>
              {!item.repo ? <p className="muted">Pick a repo first.</p> : session
                ? <button className={`btn ${session.prFlow === "awaitingReview" ? "" : "primary"}`} onClick={resume}>↻ Resume Claude</button>
                : <button className="btn primary" onClick={start}><ClaudeMark />Start Claude · {MODE_LABEL[mode]}</button>}
            </div>
          )}
        </>
      )}
      {tab === "pr" && (
        <div className="prtab">
          {!pr && <p className="empty-state">No pull request yet. Claude opens a draft PR when it finishes.</p>}
          {pr && (
            <section className={`card ${SEV[pr.overall] ?? "grey"}`}>
              <div className="card-head">
                {prBadge(pr)}
                <button className="link-btn" onClick={() => openUrl(pr.url)}>{pr.title}</button>
                <button className="btn icon" title="Refresh PR status" onClick={() => invoke("refresh_prs").catch((e) => setErr(String(e)))}>↻</button>
              </div>
              <div className="facts">
                <span className="chip">CI: {pr.ci}{pr.failingChecks.length > 0 && ` (${pr.failingChecks.join(", ")})`}</span>
                <span className="chip">{pr.unresolvedThreads} unresolved thread{pr.unresolvedThreads === 1 ? "" : "s"}</span>
                <span className="chip">Review: {pr.reviewDecision ?? (pr.changesRequested ? "CHANGES_REQUESTED" : "none")}</span>
                {pr.conflict && <span className="chip red">Merge conflict</span>}
              </div>
            </section>
          )}
          {pr && session && (session.escalations ?? []).map((x) => (
            <section key={x.id} className="card amber">
              <div className="q-head"><ClaudeMark />Claude disagrees with a review comment</div>
              <div className="meta mono">{x.path}</div>
              <blockquote>{x.comment}</blockquote>
              <p>{x.reason}</p>
              <div className="group">
                <button className="btn primary" disabled={busy} onClick={() => resolve(x.id, "doIt")}>Do it anyway</button>
                <button className="btn" disabled={busy} onClick={() => resolve(x.id, "postPushback")}>Post Claude's pushback</button>
                <button className={`btn ${own === x.id ? "on" : ""}`} disabled={busy} onClick={() => setOwn(own === x.id ? null : x.id)}>Write my own reply</button>
                <button className="btn ghost" disabled={busy} onClick={() => resolve(x.id, "ignore")}>Ignore</button>
              </div>
              {own === x.id && (
                <div className="reply">
                  <textarea value={text} onChange={(e) => setText(e.target.value)} placeholder="Your reply…" />
                  <button className="btn primary" disabled={busy || !text.trim()} onClick={() => resolve(x.id, "ownReply", text)}>Send reply</button>
                </div>
              )}
            </section>
          ))}
          {pr && session && log.length > 0 && (
            <section className="card">
              <div className="q-head">🔥 Tempering activity</div>
              <ul className="tlog">{log.map((l, n) => <li key={n}><span className="meta">{ago(l.at)}</span> {l.message}</li>)}</ul>
            </section>
          )}
        </div>
      )}
    </div>
  );
}
