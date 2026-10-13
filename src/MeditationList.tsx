import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Artifact, MedSel, Session, isMed } from "./types";
import { AgentIcon } from "./Icons";

// Artifact list + search. Reloads (debounced) on view open, query change, reload(), and when a meditation's turn ends.
export function useArtifacts(active: boolean, sessions: Record<string, Session>) {
  const [arts, setArts] = useState<Artifact[]>([]);
  const [query, setQuery] = useState("");
  const [err, setErr] = useState("");
  const [tick, setTick] = useState(0);
  const reload = () => setTick((t) => t + 1);
  useEffect(() => {
    if (!active) return;
    let dead = false;
    const t = setTimeout(() => invoke<Artifact[]>("list_artifacts", { query: query.trim() })
      .then((a) => { if (!dead) { setArts(a); setErr(""); } })
      .catch((e) => { if (!dead) setErr(String(e)); }), 200);
    return () => { dead = true; clearTimeout(t); };
  }, [active, query, tick]);
  const prev = useRef(sessions);
  useEffect(() => {
    const done = (s?: Session) => s?.status === "idle" || s?.status === "ended";
    if (Object.keys(sessions).some((k) => isMed(k) && done(sessions[k]) && !done(prev.current[k]))) reload();
    prev.current = sessions;
  }, [sessions]);
  return { arts, query, setQuery, err, reload };
}

const ago = (secs: number) => {
  const s = Math.max(0, Date.now() / 1000 - secs);
  return s < 60 ? "just now" : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago`
    : s < 7 * 86400 ? `${Math.floor(s / 86400)}d ago` : new Date(secs * 1000).toLocaleDateString();
};

type Props = { sessions: Record<string, Session>; sel: MedSel | null; onOpen: (s: MedSel | null) => void; noRepos: boolean } & ReturnType<typeof useArtifacts>;

export default function MeditationList({ sessions, sel, onOpen, noRepos, arts, query, setQuery, err: artErr }: Props) {
  const [err, setErr] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState("");
  const meds = Object.values(sessions).filter((s) => isMed(s.issueId)).sort((a, b) => (a.name ?? "").localeCompare(b.name ?? ""));
  const add = () => invoke<string>("start_meditation").then((id) => { setErr(""); onOpen({ kind: "session", id }); }).catch((e) => setErr(String(e)));
  const rename = (id: string) => {
    if (!name.trim()) return setErr("Name can't be empty");
    invoke("rename_meditation", { id, name }).then(() => { setErr(""); setEditing(null); }).catch((e) => setErr(String(e)));
  };
  const del = (id: string) => invoke("delete_meditation", { id }).then(() => { if (sel?.id === id) onOpen(null); }).catch((e) => setErr(String(e)));
  return (
    <>
      <div className="section-label">Meditations <span className="count">{meds.length}</span>
        <button className="btn ghost new" title="Start a new meditation in your repos folder" onClick={add}>+ New</button>
      </div>
      {(err || noRepos) && <div className="banner">{err || "Set your repos folder in Settings"}</div>}
      <ul className="tasks meds">
        {meds.map((s) => {
          const id = s.issueId, on = sel?.kind === "session" && sel.id === id;
          return (
            <li key={id}>
              {editing === id ? (
                <input className="rename" autoFocus value={name} onChange={(e) => setName(e.target.value)} onBlur={() => setEditing(null)}
                  onKeyDown={(e) => { if (e.key === "Enter") rename(id); if (e.key === "Escape") { setEditing(null); setErr(""); } }} />
              ) : (
                <button className={`row ${on ? "on" : ""} ${s.status === "needsInput" ? "attn" : ""}`} title="Double-click to rename"
                  onClick={() => onOpen({ kind: "session", id })} onDoubleClick={() => { setEditing(id); setName(s.name ?? ""); }}>
                  <span className="t">{s.name || id}</span>
                  <AgentIcon status={s.status} />
                </button>
              )}
              {editing !== id && <button className="x" title="Stop and remove this session (artifacts are kept)" onClick={() => del(id)}>×</button>}
            </li>
          );
        })}
        {!meds.length && <li className="empty">No sessions yet</li>}
      </ul>
      <div className="section-label">Artifacts <span className="count">{arts.length}</span></div>
      <div className="search"><input value={query} placeholder="Search artifacts…" onChange={(e) => setQuery(e.target.value)} /></div>
      {artErr && <div className="banner">{artErr}</div>}
      <ul className="tasks">
        {arts.map((a) => (
          <li key={a.path}>
            <button className={`row ${sel?.kind === "artifact" && sel.id === a.path ? "on" : ""}`} title={a.path} onClick={() => onOpen({ kind: "artifact", id: a.path })}>
              <span className="t">{a.title}</span>
              <span className="ident">{ago(a.modified)}</span>
            </button>
          </li>
        ))}
        {!arts.length && <li className="empty">{query.trim() ? "No artifacts match" : "No artifacts yet. Ask a meditation to write one."}</li>}
      </ul>
    </>
  );
}
