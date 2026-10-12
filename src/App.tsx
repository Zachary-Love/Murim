import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Settings from "./Settings";
import Caffeine from "./Caffeine";
import Updater from "./Updater";
import { Item, Repo, Session, Mode, AgentKind, LogEntry, PrMap, SEV, OVERALL, ReviewPr, sortItems, stage } from "./types";
import { StateIcon, AgentIcon, PrIcon, GearIcon, FlameIcon, TasksIcon, ReviewsIcon } from "./Icons";
import TaskPane from "./TaskPane";
import ReviewList from "./ReviewList";
import ReviewPane from "./ReviewPane";
import logo from "./assets/logo-mark.svg";

type Payload = { items: Item[]; error: string | null };
type PrPayload = { prs: PrMap; error?: string | null };
type RvPayload = { prs: ReviewPr[]; error: string | null };
type View = "tasks" | "reviews";
const savedView = (): View => { try { return localStorage.getItem("murim.view") === "reviews" ? "reviews" : "tasks"; } catch { return "tasks"; } };

export default function App() {
  const [showSettings, setShowSettings] = useState(false);
  const [p, setP] = useState<Payload>({ items: [], error: null });
  const [repos, setRepos] = useState<Repo[]>([]);
  const [sessions, setSessions] = useState<Record<string, Session>>({});
  const [mode, setMode] = useState<Mode>("normal");
  const [agent, setAgent] = useState<AgentKind>("claude");
  const [prs, setPrs] = useState<PrMap>({});
  const [prErr, setPrErr] = useState<string | null>(null);
  const [logs, setLogs] = useState<Record<string, LogEntry[]>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [view, setView] = useState<View>(savedView);
  const [openReview, setOpenReview] = useState<string | null>(null); // a ReviewPr.key; kept apart from `open` so each mode keeps its selection
  const [rv, setRv] = useState<RvPayload>({ prs: [], error: null });
  const switchView = (v: View) => { setView(v); setShowSettings(false); try { localStorage.setItem("murim.view", v); } catch {} };
  useEffect(() => {
    invoke<Record<string, Session>>("get_sessions").then(setSessions).catch(() => {});
    const us = listen<Record<string, Session>>("sessions-updated", (e) => setSessions(e.payload));
    invoke<Payload>("get_inbox").then(setP);
    invoke<Repo[]>("list_repos").then(setRepos).catch(() => {});
    const un = listen<Payload>("inbox-updated", (e) => { setP(e.payload); invoke<Repo[]>("list_repos").then(setRepos).catch(() => {}); });
    const setPp = (v: PrPayload) => { setPrs(v.prs ?? {}); setPrErr(v.error ?? null); };
    invoke<PrPayload>("get_prs").then(setPp).catch(() => {}); // ponytail: tolerate backend without get_prs
    const up = listen<PrPayload>("prs-updated", (e) => setPp(e.payload));
    invoke<RvPayload>("get_reviews").then(setRv).catch(() => {});
    const ur = listen<RvPayload>("reviews-updated", (e) => setRv(e.payload));
    const ul = listen<LogEntry>("tempering-log", (e) =>
      setLogs((l) => ({ ...l, [e.payload.issueId]: [e.payload, ...(l[e.payload.issueId] ?? [])].slice(0, 20) })));
    return () => { ul.then((f) => f()); un.then((f) => f()); us.then((f) => f()); up.then((f) => f()); ur.then((f) => f()); };
  }, []);

  // re-read when Settings closes so a new default mode / agent applies without a restart
  useEffect(() => {
    if (showSettings) return;
    invoke<{ default_permission_mode: Mode; agent: AgentKind }>("get_settings").then((s) => { setMode(s.default_permission_mode); setAgent(s.agent); }).catch(() => {});
  }, [showSettings]);
  const openItem = p.items.find((i) => i.issue.id === open);
  const requested = rv.prs.filter((r) => r.requested).length;
  const openPr = rv.prs.find((r) => r.key === openReview);

  return (
    <div className="app">
      <nav className="rail" data-tauri-drag-region>
        <button className={`btn icon ${view === "tasks" ? "on" : ""}`} title="Tasks" onClick={() => switchView("tasks")}><TasksIcon /></button>
        <button className={`btn icon ${view === "reviews" ? "on" : ""}`} title="Reviews" onClick={() => switchView("reviews")}><ReviewsIcon />{requested > 0 && <span className="count attn badge">{requested}</span>}</button>
        <div className="rail-bottom">
          <Updater />
          <Caffeine />
          <button className={`btn icon ${showSettings ? "on" : ""}`} title="Settings" onClick={() => setShowSettings(!showSettings)}><GearIcon /></button>
        </div>
      </nav>
      <aside className="sidebar">
        <header className="brand" data-tauri-drag-region>
          <h1 data-tauri-drag-region><img className="logo" src={logo} alt="" />Murim</h1>
          <button className="btn icon" title={`Refresh ${view}`} onClick={() => invoke(view === "tasks" ? "refresh_inbox" : "refresh_reviews").catch(() => {})}>↻</button>
        </header>
        {view === "reviews" ? <>
        <div className="section-label">Reviews <span className="count">{rv.prs.length}</span></div>
        <ReviewList prs={rv.prs} error={rv.error} open={showSettings ? null : openReview} onOpen={(k) => { setOpenReview(k); setShowSettings(false); }} />
        </> : <>
        <div className="section-label">My tasks <span className="count">{p.items.length}</span></div>
        {p.error && <div className="banner">{p.error}</div>}
        {prErr && <div className="banner">{prErr}</div>}
        <ul className="tasks">
          {sortItems(p.items, sessions, prs).map((i) => {
            const id = i.issue.id, se = sessions[id], pr = prs[id];
            return (
              <li key={id}>
                <button className={`row ${open === id && !showSettings ? "on" : ""} ${se?.status === "needsInput" ? "attn" : ""}`} title={i.issue.title}
                onClick={() => { setOpen(id); setShowSettings(false); }}>
                <StateIcon stage={stage(i, se, pr)} backlog={i.issue.stateType === "backlog"} />
                <span className="ident">{i.issue.identifier}</span>
                <span className="t">{i.issue.title}</span>
                <AgentIcon status={se?.status} />
                {se?.tempering && <span title="Tempering"><FlameIcon /></span>}
                {pr && <PrIcon color={SEV[pr.overall] ?? "grey"} n={se?.escalations?.length} title={`${OVERALL[pr.overall] ?? pr.overall} #${pr.number}`} />}
                </button>
              </li>
            );
          })}
          {!p.items.length && <li className="empty">No tasks in your active cycle</li>}
        </ul>
        </>}
      </aside>
      <main className="main">
        {showSettings ? <Settings /> : view === "reviews" ? (openPr
          ? <ReviewPane key={openPr.key} pr={openPr} session={sessions[openPr.sessionId]} />
          : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Select a pull request to review.</p></div>
        ) : openItem ? (
          <TaskPane key={openItem.issue.id} item={openItem} session={sessions[openItem.issue.id]} pr={prs[openItem.issue.id]} log={logs[openItem.issue.id] ?? []} repos={repos} defaultMode={mode} agent={agent} />
        ) : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Select a task to start working on it.</p></div>}
      </main>
    </div>
  );
}
