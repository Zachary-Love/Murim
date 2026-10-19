// Tempering escalations for one session: the agent disagreeing with a review comment, or Pavilion's triage asking
// the user to decide on feedback. Shared by Tasks → PR and the Pavilion inspector.
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Escalation } from "./types";

export default function Escalations({ issueId, list }: { issueId: string; list: Escalation[] }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [own, setOwn] = useState<string | null>(null); // escalation id with open textarea
  const [text, setText] = useState("");
  const resolve = async (escalationId: string, choice: string, txt: string | null = null) => {
    setBusy(true);
    try { await invoke("resolve_escalation", { issueId, escalationId, choice, text: txt }); setErr(""); setOwn(null); setText(""); } catch (e) { setErr(String(e)); }
    setBusy(false);
  };
  return <>
    {err && <div className="banner">{err}</div>}
    {list.map((x) => (
      <section key={x.id} className="card amber">
        <div className="q-head">{x.ask ? "Review feedback needs your call" : "The agent disagrees with a review comment"}</div>
        <div className="meta mono">{x.path}</div>
        <blockquote>{x.comment}</blockquote>
        <p>{x.reason}</p>
        <div className="group">
          <button className="btn primary" disabled={busy} onClick={() => resolve(x.id, "doIt")}>{x.ask ? "Have the agent do it" : "Do it anyway"}</button>
          {!x.ask && <button className="btn" disabled={busy} onClick={() => resolve(x.id, "postPushback")}>Post the agent's pushback</button>}
          {x.threadId && <button className={`btn ${own === x.id ? "on" : ""}`} disabled={busy} onClick={() => setOwn(own === x.id ? null : x.id)}>Write my own reply</button>}
          <button className="btn ghost" disabled={busy} onClick={() => resolve(x.id, "ignore")}>Ignore</button>
        </div>
        {own === x.id && (
          <div className="reply">
            <textarea value={text} onChange={(e) => setText(e.target.value)} placeholder="Your reply…" />
            <button className="btn primary" disabled={busy || !text.trim()} onClick={() => resolve(x.id, "ownReply", text)}>Send reply</button>
          </div>
        )}
      </section>
    ))}
  </>;
}
