import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrIcon } from "./Icons";
import Markdown from "./Markdown";

type Pr = { number: number; url: string; title: string; body: string; isDraft: boolean; state: string };
type Review = { base: string; commits: string[]; stat: string; diff: string; truncated: boolean; uncommitted: string[]; unpushed: number; pr: Pr | null };
type File = { name: string; lines: string[]; add: number; del: number };

// Split a unified diff into files; header lines (index/---/+++) are dropped from the body.
const files = (diff: string): File[] =>
  diff.split(/^(?=diff --git )/m).filter(Boolean).map((chunk) => {
    const all = chunk.trimEnd().split("\n");
    const name = all[0].match(/ b\/(.*)$/)?.[1] ?? all[0];
    const at = all.findIndex((l) => l.startsWith("@@") || l.startsWith("Binary"));
    const body = at < 0 ? [] : all.slice(at); // mode-only change / rename: header only
    return { name, lines: body, add: body.filter((l) => l.startsWith("+")).length, del: body.filter((l) => l.startsWith("-")).length };
  });

const cls = (l: string) => (l.startsWith("+") ? "add" : l.startsWith("-") ? "del" : l.startsWith("@@") ? "hunk" : "");

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
  if (!r) return err ? <div className="issue"><div className="banner">{err}</div></div> : <p className="empty-state">Loading diff…</p>;
  const fs = files(r.diff);
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
      <div className="facts">
        <span className="chip">{fs.length} file{fs.length === 1 ? "" : "s"} changed</span>
        <span className="chip green">+{fs.reduce((n, f) => n + f.add, 0)}</span>
        <span className="chip red">−{fs.reduce((n, f) => n + f.del, 0)}</span>
        {r.truncated && <span className="chip amber">Diff truncated</span>}
      </div>
      {!fs.length && <p className="empty">No changes vs {r.base}.</p>}
      {fs.map((f) => (
        <details key={f.name} className="file" open={f.lines.length < 400}>
          <summary><span className="mono">{f.name}</span><span className="chip green">+{f.add}</span><span className="chip red">−{f.del}</span></summary>
          <pre className="diff">{f.lines.map((l, i) => <div key={i} className={cls(l)}>{l || " "}</div>)}</pre>
        </details>
      ))}
    </div>
  );
}
