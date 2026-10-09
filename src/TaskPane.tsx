import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import { Item, Session, LABEL } from "./Inbox";

// ponytail: questions shape is backend-defined JSON; accept {questions:[{question}]}, [{question}], or strings.
const texts = (q: unknown): string[] => {
  const a = Array.isArray(q) ? q : (q as { questions?: unknown[] })?.questions;
  return (Array.isArray(a) ? a : []).map((x) => (typeof x === "string" ? x : (x as { question?: string })?.question ?? JSON.stringify(x)));
};

export default function TaskPane({ item, session, onBack }: { item: Item; session?: Session; onBack: () => void }) {
  const [prompt, setPrompt] = useState("");
  const [err, setErr] = useState("");
  const id = item.issue.id;
  const run = (cmd: string, args: object = {}) => invoke(cmd, { issueId: id, ...args }).then(() => setErr("")).catch((e) => setErr(String(e)));
  const qs = session?.status === "needsInput" ? texts(session.questions) : [];
  return (
    <div className="pane">
      <div className="pane-head">
        <button onClick={onBack}>← Back</button>
        <a className="ident" onClick={() => openUrl(item.issue.url)}>{item.issue.identifier}</a>
        <span className="pane-title">{item.issue.title}</span>
        {session && <span className={`badge ${session.status}`}>{LABEL[session.status]}</span>}
        {session && !session.ptyAlive && (
          <>
            <input placeholder="Prompt (optional)" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
            <button onClick={() => run("resume_task", { prompt: prompt || null })}>Resume</button>
          </>
        )}
        {session?.ptyAlive && <button onClick={() => run("stop_task")}>Stop</button>}
      </div>
      {err && <div className="banner">{err}</div>}
      {qs.map((q, n) => <div key={n} className="question">{q}</div>)}
      <Terminal issueId={id} />
    </div>
  );
}
