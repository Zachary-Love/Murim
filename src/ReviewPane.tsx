import { useEffect, useState, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import Markdown from "./Markdown";
import DiffFiles from "./DiffFiles";
import ReviewDraft from "./ReviewDraft";
import { Avatar } from "./ReviewList";
import { authorLabel } from "./reviewFilter";
import { ReviewPr, Session, OVERALL, SEV } from "./types";
import { AgentIcon, PrIcon, Loading } from "./Icons";
import logo from "./assets/logo-mark.svg";

type Tab = "overview" | "diff" | "agent";

export default function ReviewPane({ pr, session }: { pr: ReviewPr; session?: Session }) {
  const [tab, setTab] = useState<Tab>(session?.ptyAlive ? "agent" : "overview");
  const [err, setErr] = useState("");
  const [diff, setDiff] = useState<{ at: string; diff: string; truncated: boolean } | null>(null);
  const key = pr.key, alive = !!session?.ptyAlive, h = pr.health;
  const run = (cmd: string, args: Record<string, unknown>) => invoke(cmd, args).then((v) => { setErr(""); return v; }).catch((e) => { setErr(String(e)); throw e; });
  const start = () => run("start_review_agent", { key }).then(() => setTab("agent")).catch(() => {});

  const headAt = h.url + h.title; // ponytail: no head sha in PrHealth; refetch when the PR's url/title changes
  useEffect(() => {
    if (tab !== "diff" || diff?.at === headAt) return;
    invoke<{ diff: string; truncated: boolean }>("get_pr_diff", { key }).then((d) => setDiff({ at: headAt, ...d })).catch((e) => setErr(String(e)));
  }, [tab, headAt]);

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

      {tab === "diff" && (diff ? <div className="issue review"><DiffFiles diff={diff.diff} truncated={diff.truncated} base={pr.base} /></div> : !err && <div className="empty-state"><Loading label="Loading diff…" /></div>)}

      {/* stays mounted (hidden) on other tabs so unposted draft edits survive a tab switch */}
      <div className="review-agent" style={tab === "agent" ? undefined : { display: "none" }}>
        {tab === "agent" && (alive ? <Terminal key={pr.sessionId} issueId={pr.sessionId} /> : (
          <div className="empty-state">
            <img className="logo-lg" src={logo} alt="" />
            <p>{session ? "The review agent isn't running." : "The agent hasn't reviewed this PR yet."}</p>
            <button className="btn primary" onClick={start}>{session ? "↻ Review again" : "Review with agent"}</button>
          </div>
        ))}
        <ReviewDraft prKey={key} status={session?.status} />
      </div>
    </div>
  );
}
