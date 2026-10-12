import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Settings from "./Settings";
import Caffeine from "./Caffeine";
import Updater from "./Updater";
import { Item, Repo, Session, Mode, AgentKind, LogEntry, PrMap, SEV, OVERALL, sortItems, stage } from "./types";
import { StateIcon, AgentIcon, PrIcon, GearIcon, FlameIcon } from "./Icons";
import TaskPane from "./TaskPane";
import logo from "./assets/logo-mark.svg";

type Payload = { items: Item[]; error: string | null };
type PrPayload = { prs: PrMap; error?: string | null };

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
  useEffect(() => {
    invoke<Record<string, Session>>("get_sessions").then(setSessions).catch(() => {});
    const us = listen<Record<string, Session>>("sessions-updated", (e) => setSessions(e.payload));
    invoke<Payload>("get_inbox").then(setP);
    invoke<Repo[]>("list_repos").then(setRepos).catch(() => {});
    const un = listen<Payload>("inbox-updated", (e) => { setP(e.payload); invoke<Repo[]>("list_repos").then(setRepos).catch(() => {}); });
    const setPp = (v: PrPayload) => { setPrs(v.prs ?? {}); setPrErr(v.error ?? null); };
    invoke<PrPayload>("get_prs").then(setPp).catch(() => {}); // ponytail: tolerate backend without get_prs
    const up = listen<PrPayload>("prs-updated", (e) => setPp(e.payload));
    const ul = listen<LogEntry>("tempering-log", (e) =>
      setLogs((l) => ({ ...l, [e.payload.issueId]: [e.payload, ...(l[e.payload.issueId] ?? [])].slice(0, 20) })));
    return () => { ul.then((f) => f()); un.then((f) => f()); us.then((f) => f()); up.then((f) => f()); };
  }, []);

  // re-read when Settings closes so a new default mode / agent applies without a restart
  useEffect(() => {
    if (showSettings) return;
    invoke<{ default_permission_mode: Mode; agent: AgentKind }>("get_settings").then((s) => { setMode(s.default_permission_mode); setAgent(s.agent); }).catch(() => {});
  }, [showSettings]);
  const openItem = p.items.find((i) => i.issue.id === open);

  return (
    <div className="app">
      <aside className="sidebar">
        <header className="brand" data-tauri-drag-region>
          <h1 data-tauri-drag-region><img className="logo" src={logo} alt="" />Murim</h1>
          <button className="btn icon" title="Refresh tasks" onClick={() => invoke("refresh_inbox")}>↻</button>
        </header>
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
        <footer className="sidebar-footer">
          <Updater />
          <Caffeine />
          <button className={`btn icon ${showSettings ? "on" : ""}`} title="Settings" onClick={() => setShowSettings(!showSettings)}><GearIcon /></button>
        </footer>
      </aside>
      <main className="main">
        {showSettings ? <Settings /> : openItem ? (
          <TaskPane key={openItem.issue.id} item={openItem} session={sessions[openItem.issue.id]} pr={prs[openItem.issue.id]} log={logs[openItem.issue.id] ?? []} repos={repos} defaultMode={mode} agent={agent} />
        ) : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>Select a task to start working on it.</p></div>}
      </main>
    </div>
  );
}
