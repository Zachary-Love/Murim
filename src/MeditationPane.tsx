import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import Markdown from "./Markdown";
import { Session } from "./types";
import { AgentIcon } from "./Icons";
import logo from "./assets/logo-mark.svg";

export function SessionPane({ id, session }: { id: string; session: Session }) {
  const [err, setErr] = useState("");
  const run = (cmd: string, args: object = {}) => invoke(cmd, { issueId: id, ...args }).then(() => setErr("")).catch((e) => setErr(String(e)));
  const resume = () => run("resume_task", { prompt: null });
  return (
    <div className="pane">
      <header className="topbar" data-tauri-drag-region>
        <span className="pane-title" data-tauri-drag-region>{session.name || id}<AgentIcon status={session.status} /></span>
        <div className="topbar-right" data-tauri-drag-region>
          {session.ptyAlive
            ? <button className="btn danger" onClick={() => run("stop_task")}>■ Stop agent</button>
            : <button className="btn primary" onClick={resume}>↻ Resume agent</button>}
        </div>
      </header>
      {err && <div className="banner">{err}</div>}
      {session.ptyAlive ? <Terminal key={id} issueId={id} /> : (
        <div className="empty-state">
          <img className="logo-lg" src={logo} alt="" />
          <p>The agent isn't running for this meditation.</p>
          <button className="btn primary" onClick={resume}>↻ Resume agent</button>
        </div>
      )}
    </div>
  );
}

// `modified` changes when the list refreshes after the agent rewrites the file: re-read then.
export function ArtifactPane({ path, modified }: { path: string; modified?: number }) {
  const [text, setText] = useState<string | null>(null);
  const [err, setErr] = useState("");
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    invoke<string>("read_artifact", { path }).then((t) => { setText(t); setErr(""); }).catch(() => setErr("File not found"));
  }, [path, modified]);
  const copy = () => navigator.clipboard.writeText(path).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }).catch((e) => setErr(String(e)));
  return (
    <div className="pane">
      <header className="topbar" data-tauri-drag-region>
        <span className="pane-title mono path" title={path}>{path}</span>
        <div className="topbar-right" data-tauri-drag-region>
          <button className="btn" onClick={() => revealItemInDir(path).catch((e) => setErr(String(e)))}>Reveal in Finder</button>
          <button className="btn" onClick={copy}>{copied ? "Copied" : "Copy path"}</button>
        </div>
      </header>
      {err ? <p className="empty-state">{err}</p> : text !== null && <div className="issue"><Markdown text={text} /></div>}
    </div>
  );
}
