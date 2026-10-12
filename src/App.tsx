import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Settings from "./Settings";
import Caffeine from "./Caffeine";
import { Item, Repo, Session, Mode, LogEntry, PrMap, SEV, OVERALL, sortItems, stage } from "./types";
import { StateIcon, ClaudeIcon, PrIcon, ClaudeMark, GearIcon } from "./Icons";
import TaskPane from "./TaskPane";

type Payload = { items: Item[]; error: string | null };
type PrPayload = { prs: PrMap; error?: string | null };

export default function App() {
  const [showSettings, setShowSettings] = useState(false);
  const [p, setP] = useState<Payload>({ items: [], error: null });
  const [repos, setRepos] = useState<Repo[]>([]);
  const [sessions, setSessions] = useState<Record<string, Session>>({});
  const [mode, setMode] = useState<Mode>("normal");
  const [prs, setPrs] = useState<PrMap>({});
  const [prErr, setPrErr] = useState<string | null>(null);
  const [logs, setLogs] = useState<Record<string, LogEntry[]>>({});
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    invoke<Record<string, Session>>("get_sessions").then(setSessions).catch(() => {});
    invoke<{ defaultPermissionMode: Mode }>("get_settings").then((s) => setMode(s.defaultPermissionMode)).catch(() => {});
    const us = listen<Record<string, Session>>("sessions-updated", (e) => setSessions(e.payload));
    invoke<Payload>("get_inbox").then(setP);
    invoke<Repo[]>("list_repos").then(setRepos).catch(() => {});
    const un = listen<Payload>("inbox-updated", (e) => setP(e.payload));
    const setPp = (v: PrPayload) => { setPrs(v.prs ?? {}); setPrErr(v.error ?? null); };
    invoke<PrPayload>("get_prs").then(setPp).catch(() => {}); // ponytail: tolerate backend without get_prs
    const up = listen<PrPayload>("prs-updated", (e) => setPp(e.payload));
    const ul = listen<LogEntry>("tempering-log", (e) =>
      setLogs((l) => ({ ...l, [e.payload.issueId]: [e.payload, ...(l[e.payload.issueId] ?? [])].slice(0, 20) })));
    return () => { ul.then((f) => f()); un.then((f) => f()); us.then((f) => f()); up.then((f) => f()); };
  }, []);

  const openItem = p.items.find((i) => i.issue.id === open);

  return (
    <div className="app">
      <aside className="sidebar">
        <header className="brand">
          <h1>Murim</h1>
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
                <ClaudeIcon status={se?.status} />
                {se?.tempering && <span title="Tempering">🔥</span>}
                {pr && <PrIcon color={SEV[pr.overall] ?? "grey"} n={se?.escalations?.length} title={`${OVERALL[pr.overall] ?? pr.overall} #${pr.number}`} />}
                </button>
              </li>
            );
          })}
          {!p.items.length && <li className="empty">No tasks in your active cycle</li>}
        </ul>
        <footer className="sidebar-footer">
          <Caffeine />
          <button className={`btn icon ${showSettings ? "on" : ""}`} title="Settings" onClick={() => setShowSettings(!showSettings)}><GearIcon /></button>
        </footer>
      </aside>
      <main className="main">
        {showSettings ? <Settings /> : openItem ? (
          <TaskPane key={openItem.issue.id} item={openItem} session={sessions[openItem.issue.id]} pr={prs[openItem.issue.id]} log={logs[openItem.issue.id] ?? []} repos={repos} defaultMode={mode} />
        ) : <div className="empty-state"><ClaudeMark size={28} /><p>Select a task to see the issue and start Claude on it.</p></div>}
      </main>
    </div>
  );
}
