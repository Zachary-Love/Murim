import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import PavilionInspector from "./PavilionInspector";
import { PrIcon, RefreshIcon, RobotIcon } from "./Icons";
import { SEV, OVERALL, type Board, type Card, type LogEntry, type Session } from "./types";
import { BOARD, CHIPS, NO_FILTERS, PICK, ROLE_LABEL, SOURCE_LABEL, agentStatus, ago, chipCounts, filterCards, pickTitle, pill, prioLabel, reposOf, stats, stepOf, type Chip, type Filters } from "./pavilionView";

type Mode = "list" | "board";
const KEY = "murim.pavilion.view";
const NARROW = "(max-width: 1099px)";
const WKEY = "murim.pavilion.inspector";
const IW = 380, IW_MIN = 320, QUEUE_MIN = 360;
const two = (n: number) => String(n).padStart(2, "0");
const activate = (f: () => void) => (e: KeyboardEvent) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); f(); } };

const Si = ({ children }: { children: ReactNode }) => (
  <svg className="pv-si" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>
);
const STAT_ICON = {
  need: <Si><path d="M12 3 2 20h20L12 3z" /><path d="M12 10v4M12 17v.5" /></Si>,
  work: <Si><rect x="5" y="8" width="14" height="11" rx="2" /><path d="M12 4v4M9 13v1M15 13v1" /></Si>,
  blocked: <Si><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="12" r="2" /><path d="M6 7v10M8 12h8" /></Si>,
  open: <Si><path d="M4 6h16M4 12h16M4 18h16" /></Si>,
};

function Meta({ c }: { c: Card }) {
  const prio = prioLabel(c.priority);
  return (
    <div className="pv-ident">
      {c.ident && c.source !== "review" && <span className="pv-id">{c.ident}</span>}
      {c.pr && <span className="pv-prn"><PrIcon color={SEV[c.pr.overall] ?? "grey"} title={`${OVERALL[c.pr.overall] ?? c.pr.overall} #${c.pr.number}`} />#{c.pr.number}</span>}
      {c.repo && ((c.ident && c.source !== "review") || c.pr) && <span className="pv-sep">·</span>}
      {c.repo && <span className="pv-repo">{c.repo}</span>}
      {prio && <span className="pv-prio">{prio}</span>}
    </div>
  );
}

/** What the local model picked for this card, robot on the left. */
export const PickPill = ({ c }: { c: Card }) => c.pick ? <span className={`chip pv-pick ${PICK[c.pick].tone === "grey" ? "" : PICK[c.pick].tone} ${c.pick}`} title={pickTitle(c) || undefined}><RobotIcon />{PICK[c.pick].label}</span> : null;

const Avatar = ({ c }: { c: Card }) => c.role ? <><span className="pv-av">{ROLE_LABEL[c.role][0]}</span><span>{ROLE_LABEL[c.role]}</span></> : null;

export default function Pavilion({ board, sessions, logs, onOpen }: {
  board: Board; sessions: Record<string, Session>; logs: Record<string, LogEntry[]>; onOpen: (view: string, id: string) => void;
}) {
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [mode, setMode] = useState<Mode>(() => (localStorage.getItem(KEY) === "board" ? "board" : "list"));
  const [sel, setSel] = useState<string | null>(null);
  const [err, setErr] = useState("");
  const [drawer, setDrawer] = useState(false);
  const [narrow, setNarrow] = useState(() => window.matchMedia(NARROW).matches);
  const search = useRef<HTMLInputElement>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => { const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000); return () => clearInterval(t); }, []);
  const [dock, setDock] = useState<{ running: number; waiting: number; slots: number } | null>(null);
  useEffect(() => {
    const poll = () => invoke<{ running: number; waiting: number; slots: number }>("docker_queue").then(setDock).catch(() => {});
    poll();
    const t = setInterval(poll, 3000);
    return () => clearInterval(t);
  }, []);
  const panel = useRef<HTMLDivElement>(null);
  const [iw, setIw] = useState(() => Number(localStorage.getItem(WKEY)) || IW);
  const sizeTo = (w: number) => {
    const max = (panel.current?.clientWidth ?? 1200) - QUEUE_MIN;
    const v = Math.round(Math.max(IW_MIN, Math.min(max, w)));
    setIw(v); localStorage.setItem(WKEY, String(v));
  };
  const dragTo = (e: PointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) sizeTo(panel.current!.getBoundingClientRect().right - e.clientX);
  };

  useEffect(() => {
    const mq = window.matchMedia(NARROW);
    const on = () => { setNarrow(mq.matches); if (!mq.matches) setDrawer(false); };
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  useEffect(() => {
    if (!drawer) return;
    const on = (e: globalThis.KeyboardEvent) => { if (e.key === "Escape" && !(e.target as HTMLElement | null)?.closest("input, textarea, select, [contenteditable]")) setDrawer(false); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [drawer]);
  const closeBtn = useRef<HTMLDivElement>(null);
  useEffect(() => { if (drawer) closeBtn.current?.querySelector<HTMLElement>(".pvi-x")?.focus(); }, [drawer]);
  useEffect(() => {
    const on = (e: globalThis.KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey || t?.closest("input, textarea, select, [contenteditable]")) return;
      e.preventDefault(); search.current?.focus();
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  const run = (cmd: string, args: Record<string, unknown>) => invoke(cmd, args).then(() => { setErr(""); return true; }).catch((e) => { setErr(String(e)); return false; });
  const setView = (m: Mode) => { setMode(m); localStorage.setItem(KEY, m); };
  const set = (p: Partial<Filters>) => setFilters((f) => ({ ...f, ...p }));
  const pick = (c: Card) => { setSel(c.key); if (narrow) setDrawer(true); };

  const shown = filterCards(board.cards, filters);
  const card = board.cards.find((c) => c.key === sel) ?? shown[0] ?? board.cards[0];
  // pin what the inspector shows so board updates or search typing do not swap the card and drop edits
  useEffect(() => { if (card && card.key !== sel) setSel(card.key); }, [card?.key]); // eslint-disable-line react-hooks/exhaustive-deps
  const st = stats(board), counts = chipCounts(board.cards), repos = reposOf(board.cards);
  const updated = board.updated ? new Date(board.updated * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : "";
  const refresh = () => Promise.all(["refresh_inbox", "refresh_prs", "refresh_reviews"].map((c) => run(c, {})));

  const cards: [string, Chip, ReactNode, string, string][] = [
    ["Needs you", "needsYou", two(st.needsYou), st.plans ? `${st.plans} plan${st.plans === 1 ? "" : "s"} to approve` : "nothing waiting", st.needsYou ? "var(--amber)" : "var(--text)"],
    ["Agents working", "working", <>{two(st.working)}<span className="pv-max">/{st.max}</span></>, `${st.coding} coding · ${st.investigating} investigating · ${st.reviewing} reviewing`, "var(--green)"],
    ["Blocked", "blocked", two(st.blocked), st.inferred ? `${st.inferred} inferred` : "Linear relations", "var(--text)"],
    ["Open items", "all", two(st.open), "Linear + GitHub", "var(--text)"],
  ];
  const icons = [STAT_ICON.need, STAT_ICON.work, STAT_ICON.blocked, STAT_ICON.open];

  const row = (c: Card) => {
    const p = pill(c), step = stepOf(c.stage);
    return (
      <div key={c.key} className={`pv-row ${c.key === card?.key ? "on" : ""}`} role="button" tabIndex={0} onClick={() => pick(c)} onKeyDown={activate(() => pick(c))}>
        <div className="pv-rmain">
          <Meta c={c} />
          <div className="pv-title">{c.title}</div>
          {c.reason && <div className={`pv-reason ${p.tone}`}><span className="pv-dot" />{c.reason}</div>}
        </div>
        <div className="pv-rside">
          <div className="pv-pills"><PickPill c={c} /><span className={`chip ${p.tone === "grey" ? "" : p.tone}`}>{p.label}</span></div>
          <div className="pv-who"><Avatar c={c} />{c.updatedAt && <span className="pv-ago">{ago(c.updatedAt)}</span>}</div>
          {step >= 0 && <div className="pv-bar"><i style={{ width: `${((step + 1) / 6) * 100}%` }} /></div>}
        </div>
      </div>
    );
  };
  const mini = (c: Card) => (
    <div key={c.key} className={`pv-mini ${c.key === card?.key ? "on" : ""}`} role="button" tabIndex={0} onClick={() => pick(c)} onKeyDown={activate(() => pick(c))}>
      <Meta c={{ ...c, repo: "", priority: 0 }} />
      <div className="pv-title">{c.title}</div>
      {c.reason && <div className="pv-mreason">{c.reason}</div>}
      <PickPill c={c} />
      <div className="pv-mfoot"><span>{c.repo}</span><span>{c.role ? ROLE_LABEL[c.role] : ""}</span></div>
    </div>
  );

  const inspector = card && (
    <PavilionInspector key={card.key + ":" + (card.plan ?? "")} card={card} sessions={sessions} log={logs[card.key] ?? []} run={run} onOpen={onOpen} onClose={narrow ? () => setDrawer(false) : undefined} />
  );

  return (
    <div className="pane pavilion">
      <div className="pv-drag" data-tauri-drag-region />
      <div className="pv-scroll">
        <header className="pv-head">
          <div>
            <div className="pv-eyebrow">PAVILION</div>
            <h1>Pavilion<span>.</span></h1>
            <p>Every issue, PR and agent in one place.</p>
          </div>
          <div className="pv-actions">
{(() => { const a = agentStatus(board, now); return <span className={`chip pv-agent ${a.tone === "grey" ? "" : a.tone} ${board.thinking ? "thinking" : ""}`} title={board.localOffline ? "Judgement calls go to Claude until Ollama answers (Settings → Pavilion)" : "Pavilion's local model, from Settings → Pavilion. It re-checks the board every minute and on every change."}><RobotIcon />{a.label}</span>; })()}
            {board.error && <span className="chip red" title={board.error}>Linear error</span>}
            {!!dock?.slots && <span className={`chip ${dock.waiting ? "amber" : ""}`} title="Docker builds and runs started by agents (Settings → AI → Docker jobs at once)">docker {dock.running}/{dock.slots}{dock.waiting ? ` · ${dock.waiting} waiting` : ""}</span>}
            {updated && <span className="chip">updated {updated}</span>}
            <button className="btn" onClick={refresh}><RefreshIcon />Refresh</button>
            <button className={`btn ${board.paused ? "primary" : ""}`} title="Stop starting investigations and adopting your PRs (judgement calls keep running)" onClick={() => run("pavilion_set_paused", { paused: !board.paused })}>{board.paused ? "Resume agents" : "Pause agents"}</button>
          </div>
        </header>
        {err && <div className="banner">{err}</div>}

        <div className="pv-stats">
          {cards.map(([label, chip, n, sub, color], i) => (
            <div key={label} className={`pv-stat ${chip !== "all" && filters.chip === chip ? "on" : ""} ${chip === "needsYou" && st.needsYou > 0 ? "hot" : ""}`} role="button" tabIndex={0} onClick={() => set({ chip })} onKeyDown={activate(() => set({ chip }))}>
              <div className="pv-slabel">{icons[i]}{label}</div>
              <div className="pv-num" style={{ color }}>{n}</div>
              <div className="pv-ssub">{sub}</div>
            </div>
          ))}
        </div>

        <div className="pv-panel" ref={panel}>
          <section className="pv-queue">
            <div className="pv-qhead">
              <h2>{mode === "list" ? "Work queue" : "Work by state"} <span className="count">{board.cards.length}</span></h2>
              <div className="pv-seg">
                {(["list", "board"] as Mode[]).map((m) => <button key={m} aria-pressed={mode === m} className={mode === m ? "on" : ""} onClick={() => setView(m)}>{m === "list" ? "List" : "Board"}</button>)}
              </div>
            </div>
            <div className="pv-chips">
              {CHIPS.map(([k, label]) => <button key={k} aria-pressed={filters.chip === k} className={`chip ${filters.chip === k ? "on" : ""}`} onClick={() => set({ chip: k })}>{label} <span className="count">{counts[k]}</span></button>)}
            </div>
            <div className="pv-filters">
              <div className="pv-search">
                <input ref={search} aria-label="Search" value={filters.q} placeholder="Search issues, PRs, repos…" onChange={(e) => set({ q: e.target.value })} onKeyDown={(e) => { if (e.key === "Escape") e.currentTarget.blur(); }} />
                <kbd>/</kbd>
              </div>
              <select aria-label="Source" value={filters.source} onChange={(e) => set({ source: e.target.value as Filters["source"] })}>
                <option value="all">All sources</option>
                {Object.entries(SOURCE_LABEL).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
              </select>
              <select aria-label="Repository" value={filters.repo} onChange={(e) => set({ repo: e.target.value })}>
                <option value="all">All repositories</option>
                {repos.map((r) => <option key={r} value={r}>{r}</option>)}
              </select>
              <button className="btn ghost" onClick={() => setFilters(NO_FILTERS)}>Clear</button>
            </div>
            <div className="pv-result"><span>Showing {shown.length} of {board.cards.length}</span><span>Click a row to inspect</span></div>

            {!board.cards.length ? (
              <div className="empty-state"><p>Nothing on the board yet. Cycle issues, review PRs and running agents show up here.</p></div>
            ) : mode === "list" ? (
              shown.length ? <div className="pv-list">{shown.map(row)}</div> : (
                <div className="empty-state"><p>Nothing matches these filters</p><button className="btn" onClick={() => setFilters(NO_FILTERS)}>Clear</button></div>
              )
            ) : !shown.length ? (
              <div className="empty-state"><p>Nothing matches these filters</p><button className="btn" onClick={() => setFilters(NO_FILTERS)}>Clear</button></div>
            ) : (
              <div className="pv-board">
                {BOARD.map(([label, cols]) => {
                  const cs = shown.filter((c) => cols.includes(c.column));
                  return (
                    <div key={label} className="pv-bcol">
                      <div className="pv-bhead">{label}<span className="count">{cs.length}</span></div>
                      {cs.length ? cs.map(mini) : <div className="pv-none">Nothing here</div>}
                    </div>
                  );
                })}
              </div>
            )}
          </section>
          {!narrow && inspector && <>
            <div className="pv-split" role="separator" aria-orientation="vertical" aria-label="Resize inspector" aria-valuenow={iw} tabIndex={0} title="Drag to resize, double-click to reset"
              onPointerDown={(e) => { e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); }} onPointerMove={dragTo} onDoubleClick={() => sizeTo(IW)}
              onKeyDown={(e) => { if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); sizeTo(iw + (e.key === "ArrowLeft" ? 24 : -24)); } }} />
            <div className="pv-insp" style={{ width: iw }}>{inspector}</div>
          </>}
        </div>
      </div>
      {narrow && drawer && inspector && (
        <>
          <div className="pv-backdrop" onClick={() => setDrawer(false)} />
          <div className="pv-drawer" ref={closeBtn} role="dialog" aria-modal="true" aria-label="Inspector">{inspector}</div>
        </>
      )}
    </div>
  );
}
