import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrIcon, Loading } from "./Icons";
import Markdown from "./Markdown";
import DiffFiles from "./DiffFiles";

type Pr = { number: number; url: string; title: string; body: string; isDraft: boolean; state: string };
type Review = { base: string; commits: string[]; stat: string; diff: string; truncated: boolean; uncommitted: string[]; unpushed: number; pr: Pr | null };
export default function ReviewView({ issueId, refreshKey, onRequested }: { issueId: string; refreshKey: string; onRequested: () => void }) {
  const [r, setR] = useState<Review | null>(null);
  const [ask, setAsk] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    invoke<Review>("get_review", { issueId })
      .then((v) => { setR(v); setErr(""); })
      .catch((e) => setErr(String(e)));
  }, [issueId, refreshKey]);
  const act = async (cmd: string, args: object, then?: () => void) => {
    setBusy(true);
    try { await invoke(cmd, { issueId, ...args }); setErr(""); then?.(); } catch (e) { setErr(String(e)); }
    setBusy(false);
  };
  if (!r) return err ? <div className="issue"><div className="banner">{err}</div></div> : <div className="empty-state"><Loading label="Loading diff…" /></div>;
  const dirty = r.uncommitted.length > 0 || r.unpushed > 0;
  return (
    <div className="issue review">
      <section className="card">
        <div className="q-head">{r.pr ? "Draft pull request" : "No PR yet"}</div>
        {r.pr ? (
          <>
            <div className="card-head">
              <button className="chip" title="Open on GitHub" onClick={() => openUrl(r.pr!.url)}><PrIcon color="" title="" />#{r.pr.number} {r.pr.isDraft ? "Draft" : r.pr.state === "OPEN" ? "Ready for review" : r.pr.state.toLowerCase()} ↗</button>
              <span className="link-btn pr-title">{r.pr.title}</span>
            </div>
            <div className="pr-body">{r.pr.body ? <Markdown text={r.pr.body} /> : <p className="empty">No description</p>}</div>
          </>
        ) : <p className="muted">The agent opens a draft PR with its own title and description when it finishes.</p>}
        {dirty && <div className="banner">{r.uncommitted.length ? `${r.uncommitted.length} uncommitted file${r.uncommitted.length === 1 ? "" : "s"}` : `${r.unpushed > 1e9 ? "Branch not pushed" /* backend: no upstream = usize::MAX */ : `${r.unpushed} unpushed commit${r.unpushed === 1 ? "" : "s"}`}`}: the PR doesn't include everything yet. Ask the agent to commit and push.</div>}
        {err && <div className="banner">{err}</div>}
        {r.pr?.isDraft && (
          <div className="group">
            <button className="btn primary" disabled={busy || dirty} onClick={() => act("accept_pr", {})}>✓ Accept & mark ready for review</button>
            <span className="muted">{r.commits.length} commit{r.commits.length === 1 ? "" : "s"} vs {r.base}</span>
          </div>
        )}
        <div className="reply">
          <textarea value={ask} placeholder="Request changes from the agent…" onChange={(e) => setAsk(e.target.value)} />
          <button className="btn" disabled={busy || !ask.trim()} onClick={() => act("request_changes", { text: ask }, () => { setAsk(""); onRequested(); })}>
            Send to agent
          </button>
        </div>
      </section>
      {r.commits.length > 0 && (
        <section className="card">
          <div className="q-head">Commits</div>
          <ul className="tlog mono">{r.commits.map((c) => <li key={c}>{c}</li>)}</ul>
        </section>
      )}
      <DiffFiles diff={r.diff} truncated={r.truncated} base={r.base} />
    </div>
  );
}
