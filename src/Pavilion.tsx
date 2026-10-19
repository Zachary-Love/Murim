import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Markdown from "./Markdown";
import { SessionPane } from "./MeditationPane";
import { PrIcon } from "./Icons";
import { SEV, OVERALL, type Board, type Card, type Column, type Session } from "./types";

const COLS: [Column, string][] = [["needsYou", "Needs you"], ["working", "Working"], ["blocked", "Blocked"], ["queued", "Queued"], ["watching", "Watching"]];
const PRIO = ["", "Urgent", "High", "Medium", "Low"];
type Run = (cmd: string, args: Record<string, unknown>) => Promise<boolean>;

export default function Pavilion({ board, sessions, onOpen }: { board: Board; sessions: Record<string, Session>; onOpen: (view: string, id: string) => void }) {
  const [sel, setSel] = useState<string | null>(null);
  const [err, setErr] = useState("");
  const [showDone, setShowDone] = useState(false);
  const run: Run = (cmd, args) => invoke(cmd, args).then(() => { setErr(""); return true; }).catch((e) => { setErr(String(e)); return false; });
  // plan / investigation cards open in the side panel; everything else jumps to its own view
  const open = (c: Card) => (c.plan || c.session ? setSel(sel === c.key ? null : c.key) : onOpen(c.view, c.target));
  const card = board.cards.find((c) => c.key === sel);
  const done = board.cards.filter((c) => c.column === "done");
  const updated = board.updated ? new Date(board.updated * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : "";
  return (
    <div className="pane pavilion">
      <header className="topbar" data-tauri-drag-region>
        <h2 data-tauri-drag-region>Pavilion</h2>
        <div className="topbar-right" data-tauri-drag-region>
          {board.localOffline && <span className="chip amber" title="Judgement calls go to Claude until Ollama answers (Settings → Pavilion)">local model offline</span>}
          {board.error && <span className="chip red" title={board.error}>Linear error</span>}
          <span className="muted">{board.working}/{board.max} agents{updated && ` · updated ${updated}`}</span>
          <button className={`btn toggle ${board.paused ? "on" : ""}`} title="Stop starting new investigations" onClick={() => run("pavilion_set_paused", { paused: !board.paused })}>{board.paused ? "Paused" : "Pause"}</button>
        </div>
      </header>
      {err && <div className="banner">{err}</div>}
      <div className="pv-body">
        <div className="pv-main">
          <div className="pv-cols">
            {COLS.map(([id, label]) => {
              const cs = board.cards.filter((c) => c.column === id);
              return (
                <section key={id} className={`pv-col ${cs.length ? "" : "empty"}`}>
                  <div className="section-label">{label} <span className={`count ${id === "needsYou" && cs.length ? "attn" : ""}`}>{cs.length}</span></div>
                  {cs.map((c) => <CardView key={c.key} c={c} on={c.key === sel} onOpen={() => open(c)} onEdit={() => setSel(c.key)} run={run} />)}
                </section>
              );
            })}
          </div>
          {done.length > 0 && (
            <div className="pv-done">
              <button className="btn ghost" onClick={() => setShowDone(!showDone)}>{showDone ? "▾" : "▸"} Done <span className="count">{done.length}</span></button>
              {showDone && <div className="pv-cols">{done.map((c) => <CardView key={c.key} c={c} on={false} onOpen={() => open(c)} onEdit={() => setSel(c.key)} run={run} />)}</div>}
            </div>
          )}
          {!board.cards.length && <div className="empty-state"><p>Nothing on the board yet. Cycle issues, review PRs and running agents show up here.</p></div>}
        </div>
        {card?.plan ? <PlanPanel key={card.key + ":" + card.plan} c={card} run={run} onClose={() => setSel(null)} />
          : card?.session && sessions[card.session] ? (
            <aside className="pv-side"><SessionPane key={card.session} id={card.session} session={sessions[card.session]} title={`Investigate ${card.ident}`} noun="investigation" /></aside>
          ) : null}
      </div>
    </div>
  );
}

function CardView({ c, on, onOpen, onEdit, run }: { c: Card; on: boolean; onOpen: () => void; onEdit: () => void; run: Run }) {
  return (
    <div className={`pv-card ${on ? "on" : ""}`}>
      <div className="pv-hit" role="button" tabIndex={0} title={c.title} onClick={onOpen} onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onOpen(); } }}>
      <div className="pv-top">
        {c.ident && <span className="ident">{c.ident}</span>}
        {c.repo && <span className="muted">{c.repo}</span>}
        {c.priority > 0 && <span className="muted">{PRIO[c.priority]}</span>}
        {c.pr && <PrIcon color={SEV[c.pr.overall] ?? "grey"} title={`${OVERALL[c.pr.overall] ?? c.pr.overall} #${c.pr.number}`} />}
        {c.by && <span className="chip" title="Placed here by a judgement call">{c.by === "claude" ? "Claude-checked" : "local"}</span>}
      </div>
      <div className="t">{c.title}</div>
      <div className="pv-reason">{c.reason}</div>
      </div>
      {c.action && (
        <div className="group">
          {c.action === "approvePlan" && <><button className="btn primary" onClick={() => run("pavilion_approve", { issueId: c.key, plan: c.plan ?? "" })}>Approve plan</button><button className="btn" onClick={onEdit}>Edit</button></>}
          {c.action === "dismissBlocker" && <button className="btn" onClick={() => run("pavilion_dismiss_blocker", { issueId: c.key })}>Dismiss</button>}
          {c.action === "startNow" && <button className="btn" onClick={() => run("pavilion_start_now", { issueId: c.key })}>Start now</button>}
        </div>
      )}
    </div>
  );
}

function PlanPanel({ c, run, onClose }: { c: Card; run: Run; onClose: () => void }) {
  const [text, setText] = useState(c.plan ?? "");
  const [edit, setEdit] = useState(false);
  return (
    <aside className="pv-side pane">
      <header className="topbar">
        <span className="pane-title">{c.ident} plan</span>
        <div className="topbar-right">
          <button className="btn" onClick={() => setEdit(!edit)}>{edit ? "Preview" : "Edit"}</button>
          <button className="btn primary" disabled={!text.trim()} onClick={() => run("pavilion_approve", { issueId: c.key, plan: text }).then((ok) => ok && onClose())}>Approve → start coding</button>
          <button className="btn ghost icon" title="Close" onClick={onClose}>✕</button>
        </div>
      </header>
      <div className="pv-plan">{edit ? <textarea value={text} onChange={(e) => setText(e.target.value)} /> : <Markdown text={text} />}</div>
    </aside>
  );
}
