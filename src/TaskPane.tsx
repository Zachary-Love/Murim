import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Terminal from "./Terminal";
import { Item, Session, PrHealth, LABEL, OVERALL, SEV, prBadge, LogEntry, step } from "./Inbox";

// ponytail: questions shape is backend-defined JSON; accept {questions:[{question}]}, [{question}], or strings.
const texts = (q: unknown): string[] => {
  const a = Array.isArray(q) ? q : (q as { questions?: unknown[] })?.questions;
  return (Array.isArray(a) ? a : []).map((x) => (typeof x === "string" ? x : (x as { question?: string })?.question ?? JSON.stringify(x)));
};

export default function TaskPane({ item, session, onBack, pr, log }: { item: Item; session?: Session; pr?: PrHealth; log: LogEntry[]; onBack: () => void }) {
  const [prompt, setPrompt] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [own, setOwn] = useState<string | null>(null); // escalation id with open textarea
  const [text, setText] = useState("");
  const id = item.issue.id;
  const run = (cmd: string, args: object = {}) => invoke(cmd, { issueId: id, ...args }).then(() => setErr("")).catch((e) => setErr(String(e)));
  const resolve = async (escalationId: string, choice: string, txt: string | null = null) => {
    setBusy(true);
    await run("resolve_escalation", { escalationId, choice, text: txt });
    setBusy(false); setOwn(null); setText("");
  };
  const ago = (at: number | string) => {
    const s = Math.max(0, Math.round((Date.now() - (typeof at === "number" ? (at < 1e12 ? at * 1000 : at) : Date.parse(at))) / 1000));
    return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : `${Math.floor(s / 3600)}h ago`;
  };
  const qs = session?.status === "needsInput" ? texts(session.questions) : [];
  return (
    <div className="pane">
      <div className="pane-head">
        <button onClick={onBack}>← Back</button>
        <a className="ident" onClick={() => openUrl(item.issue.url)}>{item.issue.identifier}</a>
        <span className="pane-title">{item.issue.title}</span>
        {session && <span className={`badge ${session.status}`}>{LABEL[session.status]}</span>}
        {session && step(session)}
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
      {pr && (
        <div className={`prbox ${SEV[pr.overall] ?? "grey"}`}>
          <div>{prBadge(pr)} <a onClick={() => openUrl(pr.url)}>{pr.title}</a> <button onClick={() => invoke("refresh_prs").catch((e) => setErr(String(e)))}>↻ PR</button></div>
          <div className="meta">
            CI: {pr.ci}{pr.failingChecks.length > 0 && ` (${pr.failingChecks.join(", ")})`}
            {" · "}{pr.unresolvedThreads} unresolved thread{pr.unresolvedThreads === 1 ? "" : "s"}
            {" · "}review: {pr.reviewDecision ?? (pr.changesRequested ? "CHANGES_REQUESTED" : "none")}
            {pr.conflict && <b className="conflict"> · merge conflict</b>}
          </div>
        </div>
      )}
      {pr && session && (
        <div className="prbox temper">
          <div>
            <button className={session.tempering ? "on" : ""} onClick={() => run("set_tempering", { on: !session.tempering })}>🔥 Temper {session.tempering ? "on" : "off"}</button>
            {" "}Replies:{" "}
            <select value={session.replyMode ?? "terse"} onChange={(e) => run("set_reply_mode", { mode: e.target.value })}>
              <option value="off">Off</option>
              <option value="terse">Terse ≤5 words</option>
              <option value="full">Full</option>
            </select>
          </div>
          {(session.escalations ?? []).map((x) => (
            <div key={x.id} className="esc">
              <div className="meta">{x.path}</div>
              <blockquote>{x.comment}</blockquote>
              <div>Claude: {x.reason}</div>
              <button disabled={busy} onClick={() => resolve(x.id, "doIt")}>Do it anyway</button>
              <button disabled={busy} onClick={() => resolve(x.id, "postPushback")}>Post Claude's pushback</button>
              <button disabled={busy} onClick={() => setOwn(own === x.id ? null : x.id)}>Write my own reply</button>
              <button disabled={busy} onClick={() => resolve(x.id, "ignore")}>Ignore</button>
              {own === x.id && (
                <div>
                  <textarea value={text} onChange={(e) => setText(e.target.value)} />
                  <button disabled={busy || !text.trim()} onClick={() => resolve(x.id, "ownReply", text)}>Send</button>
                </div>
              )}
            </div>
          ))}
          {log.length > 0 && <ul className="tlog">{log.map((l, n) => <li key={n}><span className="meta">{ago(l.at)}</span> {l.message}</li>)}</ul>}
        </div>
      )}
      <Terminal issueId={id} />
    </div>
  );
}
