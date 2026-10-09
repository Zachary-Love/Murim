import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Settings from "./Settings";
import Caffeine from "./Caffeine";
import { Item, Repo, Session, Mode, LogEntry, PrMap, SEV, OVERALL, sortItems } from "./types";
import { StateIcon, ClaudeIcon, PrIcon } from "./Icons";
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
        <h1>Murim <button className="refresh" title="Refresh" onClick={() => invoke("refresh_inbox")}>↻</button></h1>
        <button onClick={() => setShowSettings(!showSettings)}>Settings</button>
        {p.error && <div className="banner">{p.error}</div>}
        {prErr && <div className="banner">{prErr}</div>}
        <ul className="tasks">
          {sortItems(p.items, sessions, prs).map((i) => {
            const id = i.issue.id, se = sessions[id], pr = prs[id];
            return (
              <li key={id} className={open === id && !showSettings ? "on" : ""} title={i.issue.title}
                onClick={() => { setOpen(id); setShowSettings(false); }}>
                <StateIcon type={i.issue.stateType} />
                <span className="ident">{i.issue.identifier}</span>
                <span className="t">{i.issue.title}</span>
                <ClaudeIcon status={se?.status} />
                {se?.tempering && <span title="Tempering">🔥</span>}
                {pr && <PrIcon color={SEV[pr.overall] ?? "grey"} n={se?.escalations?.length} title={`${OVERALL[pr.overall] ?? pr.overall} #${pr.number}`} />}
              </li>
            );
          })}
          {!p.items.length && <li className="empty">No tasks</li>}
        </ul>
        <footer className="sidebar-footer"><Caffeine /></footer>
      </aside>
      <main className="main">
        {showSettings ? <Settings /> : openItem ? (
          <TaskPane key={openItem.issue.id} item={openItem} session={sessions[openItem.issue.id]} pr={prs[openItem.issue.id]} log={logs[openItem.issue.id] ?? []} repos={repos} defaultMode={mode} />
        ) : <p className="empty">Select a task</p>}
      </main>
    </div>
  );
}
