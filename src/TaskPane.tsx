import { useEffect, useState, ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import IssueView from "./IssueView";
import ReviewView from "./ReviewView";
import { Item, Session, PrHealth, Repo, Mode, OVERALL, SEV, LogEntry, MODE_LABEL, AgentKind, repoName } from "./types";
import { AgentIcon, PrIcon, FlameIcon, FolderIcon } from "./Icons";
import logo from "./assets/logo-mark.svg";

const prBadge = (p: PrHealth) => (
  <button className={`chip ${SEV[p.overall] ?? "grey"}`} title={`Open PR: ${p.title}`} onClick={() => openUrl(p.url)}><PrIcon color="" title="" />#{p.number} {OVERALL[p.overall] ?? p.overall} ↗</button>
);
type Tab = "issue" | "agent" | "review" | "pr";

// ponytail: questions shape is backend-defined JSON; accept {questions:[{question}]}, [{question}], or strings.
const texts = (q: unknown): string[] => {
  const a = Array.isArray(q) ? q : (q as { questions?: unknown[] })?.questions;
  return (Array.isArray(a) ? a : []).map((x) => (typeof x === "string" ? x : (x as { question?: string })?.question ?? JSON.stringify(x)));
};

export default function TaskPane({ item, session, pr, log, repos, defaultMode, agent }: { item: Item; session?: Session; pr?: PrHealth; log: LogEntry[]; repos: Repo[]; defaultMode: Mode; agent: AgentKind }) {
  const [tab, setTab] = useState<Tab>(session?.status === "needsInput" ? "agent" : (session?.prFlow === "awaitingReview" || session?.prFlow === "opened") ? "review" : "issue");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [own, setOwn] = useState<string | null>(null); // escalation id with open textarea
  const [text, setText] = useState("");
  const [term, setTerm] = useState("agent"); // "agent" or a shell id
  const [shells, setShells] = useState<string[]>([]);
  const [picking, setPicking] = useState(false);
  const id = item.issue.id;
  useEffect(() => {
    const load = () => invoke<string[]>("list_shells", { issueId: id }).then((v) => {
      setShells(v);
      setTerm((t) => (t === "agent" || v.includes(t) ? t : "agent")); // closed/exited shell falls back to the agent
    }).catch(() => {});
    load();
    const un = listen("shells-changed", load);
    return () => { un.then((f) => f()); };
  }, [id]);
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
  // a running session keeps the agent it started with; otherwise Settings decides what Start launches
  const custom = (session?.agent ?? agent) === "custom";
  const start = () => run("start_task", { permissionMode: defaultMode }).then(() => setTab("agent"));
  const resume = () => run("resume_task", { prompt: null }).then(() => setTab("agent"));
  // unconfirmed repo: Claude starts in the repos folder and names it (MURIM_REPO); custom agents need a repo (or the guess)
  const guess = !item.repo || item.repo.source === "semantic" || item.repo.source === "claude";
  const canStart = !!item.repo || !custom;
  const hasWt = !!session?.worktree; // "" while Claude is still picking the repo
  const pickRepo = (path: string) => { setPicking(false); invoke("set_issue_repo", { issueId: id, path }).catch((x) => setErr(String(x))); };
  const T = ({ id, children }: { id: Tab; children: ReactNode }) => <button className={tab === id ? "on" : ""} onClick={() => setTab(id)}>{children}</button>;
  const showTerm = (t: string) => { setTerm(t); setTab("agent"); };
  const addShell = () => invoke<string>("open_shell", { issueId: id }).then((t) => { setShells((v) => [...v, t]); showTerm(t); setErr(""); }).catch((e) => setErr(String(e)));
  return (
    <div className="pane">
      <header className="topbar">
        <nav className="seg">
          <T id="issue">Issue</T>
          <T id="agent">Agent<AgentIcon status={session?.status} /></T>
          {hasWt && <T id="review">Review{(session?.prFlow === "awaitingReview" || session?.prFlow === "opened") && <span className="count attn">1</span>}</T>}
          <T id="pr">PR{pr && ` #${pr.number}`}{!!session?.escalations?.length && <span className="count">{session.escalations.length}</span>}</T>
        </nav>
        <div className="topbar-right">
          {item.resolving && <span className="chip">Matching repo…</span>}
          <span className="repo-pick">
            <button className="btn" title={guess && item.repo ? `Likely ${repoName(item)} (guess). Pick the repo` : "Pick the repo"} onClick={() => setPicking(!picking)}><FolderIcon />{!guess && repoName(item)}</button>
            {picking && <div className="popover"><button className={item.repo ? "" : "on"} onClick={() => pickRepo("")}>No repo</button>{repos.map((r) => <button key={r.path} className={r.path === item.repo?.path ? "on" : ""} onClick={() => pickRepo(r.path)}>{r.name}</button>)}</div>}
          </span>
          {alive ? (
            <button className="btn danger" onClick={() => run("stop_task")}>■ Stop agent</button>
          ) : session ? (
            <button className={`btn ${session.prFlow === "awaitingReview" ? "" : "primary"}`} onClick={resume}>↻ Resume agent</button>
          ) : (
            <button className="btn primary" disabled={!canStart} title={!canStart ? "Pick a repo first" : guess && !custom ? "Claude confirms the repo from your repos folder, then starts in its worktree" : ""} onClick={start}>Start agent</button>
          )}
          {pr && session && !custom && (
            <button className={`btn toggle ${session.tempering ? "on" : ""}`} title="Babysit the PR: rerun CI, fix failures, answer reviews (reply style is in Settings)" onClick={() => run("set_tempering", { on: !session.tempering })}><FlameIcon /> Tempering {session.tempering ? "on" : "off"}</button>
          )}
        </div>
      </header>
      <div className="tabbar">
        <button className={`tab ${tab === "agent" && term === "agent" ? "on" : ""}`} onClick={() => showTerm("agent")}>Agent<AgentIcon status={session?.status} /></button>
        {shells.map((t, n) => (
          <span key={t} className={`tab ${tab === "agent" && term === t ? "on" : ""}`}>
            <button onClick={() => showTerm(t)}>Terminal {n + 1}</button>
            <button className="x" title="Close terminal" onClick={() => invoke("close_shell", { id: t })}>×</button>
          </span>
        ))}
        <button className="tab add" title={hasWt ? "New terminal in this task's worktree" : "Start the agent first: the task has no worktree yet"} disabled={!hasWt} onClick={addShell}>+</button>
      </div>
      {err && <div className="banner">{err}</div>}
      {tab === "issue" && <IssueView issueId={id} />}
      {tab === "review" && session && <ReviewView issueId={id} refreshKey={`${session.prFlow}:${session.status}`} onRequested={() => setTab("agent")} />}
      {tab === "agent" && term !== "agent" && <Terminal key={term} issueId={term} />}
      {tab === "agent" && term === "agent" && (
        <>
          {qs.length > 0 && (
            <div className="question">
              <div className="q-head">The agent is asking — answer in the terminal below</div>
              {qs.map((q, n) => <div key={n}>{q}</div>)}
            </div>
          )}
          {alive ? <Terminal key={session?.worktree} issueId={id} /> : ( /* remount when the repo pick hands off to the worktree session */
            // ponytail: no PTY means nothing to type into; show how to launch instead of a dead xterm.
            <div className="empty-state">
              <img className="logo-lg" src={logo} alt="" />
              <p>{session ? "The agent isn't running for this task." : "The agent hasn't been started for this task."}</p>
              {!canStart ? <p className="muted">Pick a repo first.</p> : session
                ? <button className={`btn ${session.prFlow === "awaitingReview" ? "" : "primary"}`} onClick={resume}>↻ Resume agent</button>
                : <button className="btn primary" onClick={start}>Start agent{custom ? "" : ` · ${MODE_LABEL[defaultMode]}`}</button>}
            </div>
          )}
        </>
      )}
      {tab === "pr" && (
        <div className="prtab">
          {!pr && <p className="empty-state">No pull request yet. The agent opens a draft PR when it finishes.</p>}
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
              <div className="q-head">The agent disagrees with a review comment</div>
              <div className="meta mono">{x.path}</div>
              <blockquote>{x.comment}</blockquote>
              <p>{x.reason}</p>
              <div className="group">
                <button className="btn primary" disabled={busy} onClick={() => resolve(x.id, "doIt")}>Do it anyway</button>
                <button className="btn" disabled={busy} onClick={() => resolve(x.id, "postPushback")}>Post the agent's pushback</button>
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
              <div className="q-head"><FlameIcon /> Tempering activity</div>
              <ul className="tlog">{log.map((l, n) => <li key={n}><span className="meta">{ago(l.at)}</span> {l.message}</li>)}</ul>
            </section>
          )}
        </div>
      )}
    </div>
  );
}
