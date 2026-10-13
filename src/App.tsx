import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Settings, { SettingsNav, Section } from "./Settings";
import Caffeine from "./Caffeine";
import Updater from "./Updater";
import { Item, Repo, Session, Mode, AgentKind, LogEntry, PrMap, SEV, OVERALL, ReviewPr, MedSel, Automation, AutoSel, isAuto, sortItems, stage } from "./types";
import { StateIcon, AgentIcon, PrIcon, GearIcon, RefreshIcon, FlameIcon, TasksIcon, ReviewsIcon, MeditationsIcon, ClockIcon } from "./Icons";
import TaskPane from "./TaskPane";
import ReviewList from "./ReviewList";
import ReviewPane from "./ReviewPane";
import MeditationList, { useArtifacts } from "./MeditationList";
import { SessionPane, ArtifactPane } from "./MeditationPane";
import AutomationList from "./AutomationList";
import AutomationPane, { RunPane, draft } from "./AutomationPane";
import logo from "./assets/logo-mark.svg";

type Payload = { items: Item[]; error: string | null };
type PrPayload = { prs: PrMap; error?: string | null };
type RvPayload = { prs: ReviewPr[]; error: string | null };
type View = "tasks" | "reviews" | "meditations" | "automations";
const savedView = (): View => { try { const v = localStorage.getItem("murim.view"); return v === "reviews" || v === "meditations" || v === "automations" ? v : "tasks"; } catch { return "tasks"; } };

export default function App() {
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<Section>("Appearance");
  const [p, setP] = useState<Payload>({ items: [], error: null });
  const [repos, setRepos] = useState<Repo[]>([]);
  const [sessions, setSessions] = useState<Record<string, Session>>({});
  const [mode, setMode] = useState<Mode>("normal");
  const [agent, setAgent] = useState<AgentKind>("claude");
  const [badge, setBadge] = useState<"count" | "dot" | "off">("count");
  const [prs, setPrs] = useState<PrMap>({});
  const [prErr, setPrErr] = useState<string | null>(null);
  const [logs, setLogs] = useState<Record<string, LogEntry[]>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [view, setView] = useState<View>(savedView);
  const [openReview, setOpenReview] = useState<string | null>(null); // a ReviewPr.key; kept apart from `open` so each mode keeps its selection
  const [rv, setRv] = useState<RvPayload>({ prs: [], error: null });
  const [openMed, setOpenMed] = useState<MedSel | null>(null);
  const [autos, setAutos] = useState<Automation[]>([]);
  const [openAuto, setOpenAuto] = useState<AutoSel | null>(null);
  const [reposRoot, setReposRoot] = useState<string | null>(null);
  const med = useArtifacts(view === "meditations", sessions);
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
    invoke<Automation[]>("list_automations").then(setAutos).catch(() => {});
    const ua = listen<Automation[]>("automations-updated", (e) => setAutos(e.payload));
    const onBadge = (e: Event) => setBadge((e as CustomEvent).detail);
    window.addEventListener("murim-badge", onBadge);
    // menu bar item clicked: jump to that task / automation run
    const nav = listen<{ view: View; id: string }>("navigate", ({ payload: p }) => {
      switchView(p.view);
      if (p.view === "tasks") setOpen(p.id); else setOpenAuto({ kind: "run", id: p.id });
    });
    const ul = listen<LogEntry>("tempering-log", (e) =>
      setLogs((l) => ({ ...l, [e.payload.issueId]: [e.payload, ...(l[e.payload.issueId] ?? [])].slice(0, 20) })));
    return () => { nav.then((f) => f()); window.removeEventListener("murim-badge", onBadge); ul.then((f) => f()); un.then((f) => f()); us.then((f) => f()); up.then((f) => f()); ur.then((f) => f()); ua.then((f) => f()); };
  }, []);

  // re-read when Settings closes so a new default mode / agent applies without a restart
  useEffect(() => {
    if (showSettings) return;
    invoke<{ default_permission_mode: Mode; agent: AgentKind; review_badge: typeof badge; repos_root: string }>("get_settings").then((s) => { setMode(s.default_permission_mode); setAgent(s.agent); setBadge(s.review_badge); setReposRoot(s.repos_root); }).catch(() => {});
  }, [showSettings]);
  const openItem = p.items.find((i) => i.issue.id === open);
  const requested = rv.prs.filter((r) => r.requested).length;
  const openPr = rv.prs.find((r) => r.key === openReview);
  const autoWaiting = Object.values(sessions).some((s) => isAuto(s.issueId) && s.status === "needsInput");
  const runAuto = openAuto?.kind === "run" ? autos.find((a) => a.runs.some((r) => r.sessionId === openAuto.id)) : undefined;
  const selAuto = showSettings ? null : openAuto?.kind === "automation" ? openAuto.id : runAuto?.id ?? null;
  const editAuto = openAuto?.kind === "automation" ? (openAuto.id ? autos.find((a) => a.id === openAuto.id) : draft()) : undefined;
  const medSession = openMed?.kind === "session" ? sessions[openMed.id] : undefined;

  return (
    <div className="app">
      <nav className="rail" data-tauri-drag-region>
        <button className={`btn icon ${!showSettings && view === "tasks" ? "on" : ""}`} title="Tasks" onClick={() => switchView("tasks")}><TasksIcon /></button>
        <button className={`btn icon ${!showSettings && view === "reviews" ? "on" : ""}`} title="Reviews" onClick={() => switchView("reviews")}><ReviewsIcon />{requested > 0 && badge !== "off" && (badge === "dot" ? <span className="dot-req badge-dot" /> : <span className="count attn badge">{requested}</span>)}</button>
        <button className={`btn icon ${!showSettings && view === "meditations" ? "on" : ""}`} title="Meditations" onClick={() => switchView("meditations")}><MeditationsIcon /></button>
        <button className={`btn icon ${!showSettings && view === "automations" ? "on" : ""}`} title="Automations" onClick={() => switchView("automations")}><ClockIcon />{autoWaiting && <span className="dot-req badge-dot" />}</button>
        <div className="rail-bottom">
          <Updater />
          <Caffeine />
          <button className={`btn icon ${showSettings ? "on" : ""}`} title="Settings" onClick={() => setShowSettings(!showSettings)}><GearIcon /></button>
        </div>
      </nav>
      <aside className="sidebar">
        <header className="brand" data-tauri-drag-region>
          <h1 data-tauri-drag-region><img className="logo" src={logo} alt="" />Murim</h1>
        </header>
        {showSettings ? (
          <SettingsNav tab={settingsTab} onTab={setSettingsTab} />
        ) : view === "automations" ? (
          <AutomationList autos={autos} sessions={sessions} selAuto={selAuto} onOpen={(a) => { setOpenAuto(a); setShowSettings(false); }} />
        ) : view === "meditations" ? (
          <MeditationList {...med} sessions={sessions} sel={showSettings ? null : openMed} onOpen={(m) => { setOpenMed(m); setShowSettings(false); }} noRepos={reposRoot === ""} />
        ) : view === "reviews" ? <>
        <div className="section-label">Reviews <span className="count">{rv.prs.length}</span><button className="btn ghost new" title="Refresh reviews" onClick={() => invoke("refresh_reviews").catch(() => {})}><RefreshIcon /></button></div>
        <ReviewList prs={rv.prs} error={rv.error} open={showSettings ? null : openReview} onOpen={(k) => { setOpenReview(k); setShowSettings(false); }} />
        </> : <>
        <div className="section-label">My tasks <span className="count">{p.items.length}</span><button className="btn ghost new" title="Refresh tasks" onClick={() => invoke("refresh_inbox").catch(() => {})}><RefreshIcon /></button></div>
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
        {showSettings ? <Settings tab={settingsTab} /> : view === "automations" ? (
          editAuto ? <AutomationPane key={openAuto!.id} auto={editAuto} sessions={sessions} onOpen={setOpenAuto}
            onSaved={(a) => { setAutos((l) => l.some((x) => x.id === a.id) ? l.map((x) => x.id === a.id ? a : x) : [...l, a]); setOpenAuto({ kind: "automation", id: a.id }); }} />
          : openAuto?.kind === "run" ? <RunPane key={openAuto.id} id={openAuto.id} auto={runAuto} session={sessions[openAuto.id]} />
          : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Select an automation or create one.</p></div>
        ) : view === "meditations" ? (
          medSession ? <SessionPane key={openMed!.id} id={openMed!.id} session={medSession} />
          : openMed?.kind === "artifact" ? <ArtifactPane key={openMed.id} path={openMed.id} modified={med.arts.find((a) => a.path === openMed.id)?.modified} />
          : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Start a meditation or open an artifact.</p></div>
        ) : view === "reviews" ? (openPr
          ? <ReviewPane key={openPr.key} pr={openPr} session={sessions[openPr.sessionId]} />
          : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Select a pull request to review.</p></div>
        ) : openItem ? (
          <TaskPane key={openItem.issue.id} item={openItem} session={sessions[openItem.issue.id]} pr={prs[openItem.issue.id]} log={logs[openItem.issue.id] ?? []} repos={repos} defaultMode={mode} agent={agent} />
        ) : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Select a task to start working on it.</p></div>}
      </main>
    </div>
  );
}
