import { useEffect, useState, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import Markdown from "./Markdown";
import DiffFiles from "./DiffFiles";
import { Avatar } from "./ReviewList";
import { authorLabel } from "./reviewFilter";
import { ReviewPr, Session, Draft, OVERALL, SEV } from "./types";
import { AgentIcon, PrIcon } from "./Icons";
import logo from "./assets/logo-mark.svg";

type Tab = "overview" | "diff" | "agent";
const EVENTS: [Draft["event"], string][] = [["COMMENT", "Comment"], ["APPROVE", "Approve"], ["REQUEST_CHANGES", "Request changes"]];

export default function ReviewPane({ pr, session }: { pr: ReviewPr; session?: Session }) {
  const [tab, setTab] = useState<Tab>(session?.ptyAlive ? "agent" : "overview");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [diff, setDiff] = useState<{ at: string; diff: string; truncated: boolean } | null>(null);
  const [disk, setDisk] = useState<Draft | null>(null); // last draft read from .murim/review.json
  const [diskErr, setDiskErr] = useState("");
  const [edit, setEdit] = useState<Draft | null>(null); // user's edits; null = follow the file
  const [base, setBase] = useState(""); // the file version the edits started from
  const [posted, setPosted] = useState("");
  const key = pr.key, alive = !!session?.ptyAlive, h = pr.health;
  const run = (cmd: string, args: Record<string, unknown>) => invoke(cmd, args).then((v) => { setErr(""); return v; }).catch((e) => { setErr(String(e)); throw e; });
  const start = () => run("start_review_agent", { key }).then(() => setTab("agent")).catch(() => {});

  const headAt = h.url + h.title; // ponytail: no head sha in PrHealth; refetch when the PR's url/title changes
  useEffect(() => {
    if (tab !== "diff" || diff?.at === headAt) return;
    invoke<{ diff: string; truncated: boolean }>("get_pr_diff", { key }).then((d) => setDiff({ at: headAt, ...d })).catch((e) => setErr(String(e)));
  }, [tab, headAt]);

  useEffect(() => {
    if (tab !== "agent") return;
    const load = () => invoke<Draft | null>("get_review_draft", { key }).then((d) => { setDisk(d); setDiskErr(""); }).catch((e) => setDiskErr(String(e)));
    load();
    const t = setInterval(load, 4000);
    return () => clearInterval(t);
  }, [tab, session?.status]);

  const draft = edit ?? disk;
  const change = (d: Draft) => { if (!edit) setBase(JSON.stringify(disk)); setEdit(d); setPosted(""); };
  const stale = !!edit && base !== JSON.stringify(disk);
  const post = async () => { // on success keep showing what was posted
    setBusy(true);
    try { await run("post_review", { key, draft }); setPosted(EVENTS.find((e) => e[0] === draft!.event)?.[1] ?? "Review"); setEdit(draft); setBase(JSON.stringify(disk)); } catch { /* error banner; draft kept */ }
    setBusy(false);
  };

  const T = ({ id, children }: { id: Tab; children: ReactNode }) => <button className={tab === id ? "on" : ""} onClick={() => setTab(id)}>{children}</button>;
  return (
    <div className="pane">
      <header className="topbar" data-tauri-drag-region>
        <nav className="seg">
          <T id="overview">Overview</T>
          <T id="diff">Diff</T>
          <T id="agent">Agent<AgentIcon status={session?.status} /></T>
        </nav>
        <div className="topbar-right" data-tauri-drag-region>
          <button className={`chip ${SEV[h.overall] ?? "grey"}`} title={`Open on GitHub: ${h.title}`} onClick={() => openUrl(h.url)}><PrIcon color="" title="" />{pr.repo}#{pr.number} ↗</button>
          {alive
            ? <button className="btn danger" onClick={() => run("stop_task", { issueId: pr.sessionId }).catch(() => {})}>■ Stop agent</button>
            : <button className="btn primary" onClick={start}>{session ? "↻ Review again" : "Review with agent"}</button>}
          {session && <button className="btn ghost" title="Remove the review checkout and its agent session" onClick={() => run("dismiss_review", { key }).catch(() => {})}>Dismiss</button>}
        </div>
      </header>
      {err && <div className="banner">{err}</div>}

      {tab === "overview" && (
        <div className="issue rv-overview">
          <h2>{h.title}</h2>
          <div className="facts">
            <span className="chip"><Avatar url={pr.author.kind === "linear" ? pr.author.avatarUrl : null} label={authorLabel(pr.author)} unlinked={pr.author.kind === "unlinked"} />{authorLabel(pr.author)}</span>
            {pr.requested && <span className="chip req">Review requested</span>}
            <span className={`chip ${SEV[h.overall] ?? ""}`}>{OVERALL[h.overall] ?? h.overall}</span>
            <span className="chip">CI: {h.ci}{h.failingChecks.length > 0 && ` (${h.failingChecks.join(", ")})`}</span>
            {h.unresolvedThreads > 0 && <span className="chip">{h.unresolvedThreads} unresolved thread{h.unresolvedThreads === 1 ? "" : "s"}</span>}
            {pr.issue && <button className="chip" title="Open in Linear" onClick={() => openUrl(pr.issue!.url)}>{pr.issue.identifier} ↗</button>}
          </div>
          <div className="meta mono">{pr.owner}/{pr.repo} · {pr.branch} → {pr.base}</div>
          {pr.body ? <Markdown text={pr.body} /> : <p className="empty">No description</p>}
        </div>
      )}

      {tab === "diff" && (diff ? <div className="issue review"><DiffFiles diff={diff.diff} truncated={diff.truncated} base={pr.base} /></div> : !err && <p className="empty-state">Loading diff…</p>)}

      {tab === "agent" && (
        <div className="review-agent">
          {alive ? <Terminal key={pr.sessionId} issueId={pr.sessionId} /> : (
            <div className="empty-state">
              <img className="logo-lg" src={logo} alt="" />
              <p>{session ? "The review agent isn't running." : "The agent hasn't reviewed this PR yet."}</p>
              <button className="btn primary" onClick={start}>{session ? "↻ Review again" : "Review with agent"}</button>
            </div>
          )}
          <section className="card draft">
            <div className="q-head">Draft review{stale && <button className="link-btn reload" onClick={() => setEdit(null)}>The agent updated the draft · Reload</button>}</div>
            {diskErr && <div className="banner">{diskErr.split("\n")[0]}{diskErr.includes("\n") && <pre>{diskErr.split("\n").slice(1).join("\n")}</pre>}</div>}
            {!draft && !diskErr && <p className="muted">Waiting for the agent to write .murim/review.json…</p>}
            {draft && (
              <>
                <div className="draft-body">
                <textarea value={draft.summary} placeholder="Summary" onChange={(e) => change({ ...draft, summary: e.target.value })} />
                {draft.comments.map((c, i) => (
                  <div key={i} className="draft-comment">
                    <div className="meta mono">{c.path}:{c.line}<button className="x" title="Remove comment" onClick={() => change({ ...draft, comments: draft.comments.filter((_, j) => j !== i) })}>×</button></div>
                    <textarea value={c.body} onChange={(e) => change({ ...draft, comments: draft.comments.map((x, j) => (j === i ? { ...x, body: e.target.value } : x)) })} />
                  </div>
                ))}
                </div>
                <div className="group">
                  <select value={draft.event} onChange={(e) => change({ ...draft, event: e.target.value as Draft["event"] })}>
                    {EVENTS.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
                  </select>
                  <button className="btn primary" disabled={busy || !!diskErr || !!posted} onClick={post}>Post: {EVENTS.find((e) => e[0] === draft.event)?.[1]}</button>
                  <span className="muted">{posted ? `Posted (${posted})` : "Nothing is posted until you click"}</span>
                </div>
              </>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
