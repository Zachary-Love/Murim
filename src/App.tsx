import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Settings from "./Settings";
import Caffeine from "./Caffeine";
import Inbox, { Item, Repo, repoName } from "./Inbox";

type Payload = { items: Item[]; error: string | null };
const NONE = "\0none";

export default function App() {
  const [showSettings, setShowSettings] = useState(false);
  const [p, setP] = useState<Payload>({ items: [], error: null });
  const [repos, setRepos] = useState<Repo[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  useEffect(() => {
    invoke<Payload>("get_inbox").then(setP);
    invoke<Repo[]>("list_repos").then(setRepos).catch(() => {});
    const un = listen<Payload>("inbox-updated", (e) => setP(e.payload));
    return () => { un.then((f) => f()); };
  }, []);

  const counts = new Map<string, number>();
  for (const i of p.items) counts.set(repoName(i) || NONE, (counts.get(repoName(i) || NONE) ?? 0) + 1);
  const groups = [...counts].sort(([a], [b]) => (a === NONE ? 1 : b === NONE ? -1 : a.localeCompare(b)));
  const shown = sel ? p.items.filter((i) => (repoName(i) || NONE) === sel) : p.items;

  return (
    <div className="app">
      <aside className="sidebar">
        <h1>Murim <button className="refresh" title="Refresh" onClick={() => invoke("refresh_inbox")}>↻</button></h1>
        <button onClick={() => setShowSettings(!showSettings)}>Settings</button>
        <div className="label">Repos</div>
        <ul className="repos">
          <li className={sel === null ? "on" : ""} onClick={() => { setSel(null); setShowSettings(false); }}>All <span>{p.items.length}</span></li>
          {groups.map(([g, n]) => (
            <li key={g} className={sel === g ? "on" : ""} onClick={() => { setSel(g); setShowSettings(false); }}>
              {g === NONE ? "No repo" : g} <span>{n}</span>
            </li>
          ))}
        </ul>
        <footer className="sidebar-footer"><Caffeine /></footer>
      </aside>
      <main className="main">
        {showSettings ? <Settings /> : (
          <>
            {p.error && <div className="banner">{p.error}</div>}
            <Inbox items={shown} repos={repos} />
          </>
        )}
      </main>
    </div>
  );
}
