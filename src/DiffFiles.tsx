// Unified-diff viewer: per-file collapsible blocks with +/- counts. Used by the task Review tab and the PR review pane.
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

export default function DiffFiles({ diff, truncated, base }: { diff: string; truncated: boolean; base?: string }) {
  const fs = files(diff);
  return (
    <>
      <div className="facts">
        <span className="chip">{fs.length} file{fs.length === 1 ? "" : "s"} changed</span>
        <span className="chip green">+{fs.reduce((n, f) => n + f.add, 0)}</span>
        <span className="chip red">−{fs.reduce((n, f) => n + f.del, 0)}</span>
        {truncated && <span className="chip amber">Diff truncated</span>}
      </div>
      {!fs.length && <p className="empty">No changes{base && ` vs ${base}`}.</p>}
      {fs.map((f) => (
        <details key={f.name} className="file" open={f.lines.length < 400}>
          <summary><span className="mono">{f.name}</span><span className="chip green">+{f.add}</span><span className="chip red">−{f.del}</span></summary>
          <pre className="diff">{f.lines.map((l, i) => <div key={i} className={cls(l)}>{l || " "}</div>)}</pre>
        </details>
      ))}
    </>
  );
}
