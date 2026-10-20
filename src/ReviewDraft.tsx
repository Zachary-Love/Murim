// The agent's draft review (.murim/review.json): polled, editable, posted only on click. Used by Reviews → Agent and Pavilion's inspector.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Draft } from "./types";

const EVENTS: [Draft["event"], string][] = [["COMMENT", "Comment"], ["APPROVE", "Approve"], ["REQUEST_CHANGES", "Request changes"]];

export default function ReviewDraft({ prKey: key, status }: { prKey: string; status?: string }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [disk, setDisk] = useState<Draft | null>(null); // last draft read from .murim/review.json
  const [diskErr, setDiskErr] = useState("");
  const [edit, setEdit] = useState<Draft | null>(null); // user's edits; null = follow the file
  const [base, setBase] = useState(""); // the file version the edits started from
  const [posted, setPosted] = useState("");

  useEffect(() => {
    const load = () => invoke<Draft | null>("get_review_draft", { key }).then((d) => { setDisk(d); setDiskErr(""); }).catch((e) => setDiskErr(String(e)));
    load();
    const t = setInterval(load, 4000);
    return () => clearInterval(t);
  }, [key, status]);

  const draft = edit ?? disk;
  const change = (d: Draft) => { if (!edit) setBase(JSON.stringify(disk)); setEdit(d); setPosted(""); };
  const stale = !!edit && base !== JSON.stringify(disk);
  const post = async () => { // on success keep showing what was posted
    setBusy(true);
    try { await invoke("post_review", { key, draft }); setErr(""); setPosted(EVENTS.find((e) => e[0] === draft!.event)?.[1] ?? "Review"); setEdit(draft); setBase(JSON.stringify(disk)); } catch (e) { setErr(String(e)); /* draft kept */ }
    setBusy(false);
  };

  return (
    <section className="card draft">
      <div className="q-head">Draft review{stale && <button className="link-btn reload" onClick={() => setEdit(null)}>The agent updated the draft · Reload</button>}</div>
      {err && <div className="banner">{err}</div>}
      {diskErr && <div className="banner">{diskErr.split("\n")[0]}{diskErr.includes("\n") && <pre>{diskErr.split("\n").slice(1).join("\n")}</pre>}</div>}
      {!draft && !diskErr && <p className="muted">Waiting for the agent to write .murim/review.json…</p>}
      {draft && (
        <>
          <div className="draft-body">
          <textarea value={draft.summary} placeholder="Summary" onChange={(e) => change({ ...draft, summary: e.target.value })} />
          {draft.comments.map((c, i) => (
            <div key={i} className="draft-comment">
              <div className="meta mono">{c.path}:{c.line}<button className="x" title="Remove comment" onClick={() => change({ ...draft, comments: draft.comments.filter((_, j) => j !== i) })}>×</button></div>
              <textarea value={c.body} onChange={(e) => change({ ...draft, comments: draft.comments.map((x, j) => (j === i ? { ...x, body: e.target.value } : x)) })} />
            </div>
          ))}
          </div>
          <div className="group">
            <select value={draft.event} onChange={(e) => change({ ...draft, event: e.target.value as Draft["event"] })}>
              {EVENTS.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
            </select>
            <button className="btn primary" disabled={busy || !!diskErr || !!posted} onClick={post}>Post: {EVENTS.find((e) => e[0] === draft.event)?.[1]}</button>
            <span className="muted">{posted ? `Posted (${posted})` : "Nothing is posted until you click"}</span>
          </div>
        </>
      )}
    </section>
  );
}
