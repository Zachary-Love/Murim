// Pavilion's right-hand inspector: pipeline, decision, judgement, links, checks, plan, terminal and activity for one card.
import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Card, LogEntry, Session } from "./types";
import { OVERALL, SEV } from "./types";
import { PIPELINE, ROLE_LABEL, SOURCE_LABEL, VIEW_LABEL, pill, stepOf } from "./pavilionView";
import { SessionPane } from "./MeditationPane";
import ReviewView from "./ReviewView";
import Markdown from "./Markdown";
import { PrIcon } from "./Icons";

type Tab = "overview" | "plan" | "review" | "terminal" | "activity";
const TABS: [Tab, string][] = [["overview", "Overview"], ["plan", "Plan"], ["review", "Review"], ["terminal", "Terminal"], ["activity", "Activity"]];
const TONE: Record<string, string> = { amber: "amber", red: "red", green: "green", blue: "blue", grey: "" };

export default function PavilionInspector({ card, sessions, log, run, onOpen, onClose }: {
  card: Card; sessions: Record<string, Session>; log: LogEntry[];
  run: (cmd: string, args: Record<string, unknown>) => Promise<boolean>;
  onOpen: (view: string, id: string) => void; onClose?: () => void;
}) {
  const [text, setText] = useState(card.plan ?? "");
  const [edit, setEdit] = useState(false);
  const [busy, setBusy] = useState(false);
  const c = card, pr = c.pr, st = pill(c), step = stepOf(c.stage);
  const hasTerm = !!(c.session && sessions[c.session]);
  // the coding session's draft PR, reviewed here like Tasks → Review
  const coding = c.source === "issue" && c.session === c.key ? sessions[c.key] : undefined;
  const toReview = coding?.prFlow === "awaitingReview" || coding?.prFlow === "opened";
  const [tab, setTab] = useState<Tab>(c.plan ? "plan" : toReview && coding?.status !== "needsInput" ? "review" : hasTerm && c.column === "needsYou" ? "terminal" : "overview");
  const tabs = TABS.filter(([k]) => k === "overview" || (k === "plan" ? !!c.plan : k === "review" ? !!coding?.worktree : k === "terminal" ? hasTerm : log.length > 0));
  const cur = tabs.some(([k]) => k === tab) ? tab : "overview";
  const view = VIEW_LABEL[c.view] ?? c.view;
  const exec = async (cmd: string, args: Record<string, unknown>) => { setBusy(true); try { await run(cmd, args); } finally { setBusy(false); } };
  const approve = () => exec("pavilion_approve", { issueId: c.key, plan: text });
  const start = () => exec("pavilion_start_now", { issueId: c.key });
  const dismiss = () => exec("pavilion_dismiss_blocker", { issueId: c.key });
  const open = () => onOpen(c.view, c.target);
  const canStart = c.action === "startNow" || c.column === "queued";
  const primary = c.action === "approvePlan" ? { label: "Approve plan", go: approve, off: !text.trim() }
    : canStart ? { label: "Start now", go: start, off: false }
    : c.action === "dismissBlocker" ? { label: "Dismiss blocker", go: dismiss, off: false } : null;
  const failing = pr?.failingChecks ?? [];
  const ci = pr?.ci;
  const allPass = !!pr && !failing.length && (ci === "passing" || pr.overall === "readyToMerge");

  return (
    <aside className="pvi">
      <div className="pvi-head">
        <span className="pvi-cap">INSPECTOR</span>
        <span className="chip">{SOURCE_LABEL[c.source].replace(/s$/, "")}</span>
        {onClose && <button className="btn ghost icon pvi-x" title="Close" aria-label="Close inspector" onClick={onClose}>✕</button>}
      </div>
      <div className="pvi-scroll">
        <div className="pvi-top">
          <div className="pvi-ident">
            {c.source === "issue" && c.ident && <span>{c.ident}</span>}
            {pr && <span><PrIcon color={SEV[pr.overall] ?? "grey"} title={OVERALL[pr.overall] ?? pr.overall} /> #{pr.number}</span>}
            {c.repo && <span>{c.repo}</span>}
          </div>
          <h2 className="pvi-title">{c.title}</h2>
          <span className={`chip ${TONE[st.tone]}`}>{st.label}</span>
          {c.summary && <p className="pvi-sum">{c.summary}</p>}
        </div>
        <div className="pvi-tabs" role="tablist">
          {tabs.map(([k, l]) => <button key={k} role="tab" aria-selected={cur === k} className={cur === k ? "on" : ""} onClick={() => setTab(k)}>{l}</button>)}
        </div>

        {cur === "overview" && <>
          {c.stage && (
            <section className="pvi-sec">
              <div className="pvi-row"><span className="pvi-cap">Pipeline</span><span className="pvi-dim">{Math.round(step / 5 * 100)}% through</span></div>
              <ol className="pvi-steps">
                {PIPELINE.map(([k, l], i) => (
                  <li key={k} className={i < step ? "done" : i === step ? "now" : ""} aria-current={i === step ? "step" : undefined}>
                    <span className="pvi-dot">{i < step ? "✓" : i + 1}</span><span className="pvi-lbl">{l}</span>
                  </li>
                ))}
              </ol>
            </section>
          )}
          {c.column === "needsYou" && (
            <section className="pvi-sec">
              <div className="pvi-box amber">
                <div className="pvi-cap">Your decision</div>
                <p>{c.reason}</p>
                <div className="pvi-btns">
                  {c.action === "approvePlan" && <><button className="btn primary" disabled={busy || !text.trim()} onClick={approve}>Approve plan</button><button className="btn" onClick={() => setTab("plan")}>View plan</button></>}
                  {c.action === "startNow" && <button className="btn primary" disabled={busy} onClick={start}>Investigate anyway</button>}
                  {!c.action && toReview && <button className="btn primary" onClick={() => setTab("review")}>Review PR</button>}
                  {!c.action && !toReview && (hasTerm ? <button className="btn primary" onClick={() => setTab("terminal")}>Open terminal</button> : <button className="btn primary" onClick={open}>Open in {view}</button>)}
                </div>
              </div>
            </section>
          )}
          {c.action === "dismissBlocker" && (
            <section className="pvi-sec">
              <div className="pvi-box red"><div className="pvi-cap">Blocked</div><p>{c.reason}</p>
                <div className="pvi-btns"><button className="btn" disabled={busy} onClick={dismiss}>Dismiss blocker</button></div></div>
            </section>
          )}
          {c.column === "queued" && (
            <section className="pvi-sec">
              <div className="pvi-box"><p className="pvi-dim">Waiting for a free agent slot</p>
                <div className="pvi-btns"><button className="btn" disabled={busy} onClick={start}>Start now</button></div></div>
            </section>
          )}
          {c.verdict && (
            <section className="pvi-sec">
              <div className="pvi-row"><span className="pvi-cap">Judgement</span><span className="pvi-dim">{Math.round(c.verdict.confidence * 100)}% confidence</span></div>
              <p className="pvi-text">{c.verdict.reason}</p>
              <div className="pvi-chips"><span className="chip">{c.verdict.by === "claude" ? "Claude-checked" : "Local model"}</span><span className="chip">From local state</span></div>
            </section>
          )}
          <section className="pvi-sec">
            <div className="pvi-cap">Linked</div>
            <dl className="pvi-links">
              {c.source === "issue" && c.ident && <Link k="Linear issue" v={c.ident} href={c.url} />}
              {pr && <Link k="Pull request" v={`#${pr.number}`} href={pr.url} />}
              {c.repo && <Link k="Repository" v={c.repo} />}
              {c.role && <Link k="Worker" v={ROLE_LABEL[c.role]} />}
              {c.role && <Link k="Runtime" v="Claude Code" />}
            </dl>
          </section>
          {pr && (
            <section className="pvi-sec">
              <div className="pvi-row"><span className="pvi-cap">Checks</span><span className={`pvi-st ${SEV[pr.overall] ?? ""}`}>{OVERALL[pr.overall] ?? pr.overall}</span></div>
              <ul className="pvi-checks">
                {failing.map((n) => <li key={n}><span>{n}</span><span className="pvi-st red">Failing</span></li>)}
                {pr.unresolvedThreads > 0 && <li><span>Unresolved comments</span><span className="pvi-st amber">{pr.unresolvedThreads}</span></li>}
                {pr.conflict && <li><span>Merge conflict</span><span className="pvi-st red">Yes</span></li>}
                {allPass && <li><span>All checks passing</span><span className="pvi-st green">✓</span></li>}
              </ul>
            </section>
          )}
        </>}

        {cur === "plan" && c.plan && (
          <section className="pvi-sec pvi-plan">
            <div className="pvi-btns"><button className="btn" onClick={() => setEdit(!edit)}>{edit ? "Preview" : "Edit"}</button></div>
            {edit ? <textarea value={text} onChange={(e) => setText(e.target.value)} aria-label="Plan" /> : <Markdown text={text} />}
          </section>
        )}
        {cur === "review" && coding && (
          <div className="pvi-term"><ReviewView issueId={c.key} refreshKey={`${coding.prFlow}:${coding.status}`} onRequested={() => setTab("terminal")} /></div>
        )}
        {cur === "terminal" && hasTerm && c.session && (
          <div className="pvi-term"><SessionPane id={c.session} session={sessions[c.session]} title={c.session.startsWith("investigation:") ? `Investigate ${c.ident || c.title}` : c.ident || c.title} noun={c.session.startsWith("investigation:") ? "investigation" : c.source === "review" ? "review" : c.source === "agent" ? "run" : "task"} /></div>
        )}
        {cur === "activity" && (
          <ul className="pvi-sec pvi-log">
            {[...log].reverse().map((e, i) => (
              <li key={i}><time>{new Date(e.at).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}</time><span>{e.message}</span></li>
            ))}
          </ul>
        )}
      </div>
      <div className="pvi-foot">
        <button className="btn" onClick={open}>Open in {view}</button>
        {!c.action && hasTerm && <button className={`btn ${toReview ? "" : "primary"}`} onClick={() => setTab("terminal")}>Open terminal</button>}
        {primary && <button className="btn primary" disabled={busy || primary.off} onClick={primary.go}>{primary.label}</button>}
      </div>
    </aside>
  );
}

function Link({ k, v, href }: { k: string; v: string; href?: string }) {
  return <div><dt>{k}</dt><dd>{href ? <button className="link-btn pvi-mono" onClick={() => openUrl(href)} title="Open">{v} ↗</button> : <span className="pvi-mono">{v}</span>}</dd></div>;
}
