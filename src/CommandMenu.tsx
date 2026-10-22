// ⌘K: Linear-style command menu. Type to filter, ↑↓ to pick, ↵ to run, esc to close.
import { useEffect, useRef, useState } from "react";

export type Command = { id: string; label: string; group: string; hint?: string; run: () => void };

/** Every typed word must appear in the label or group (case-insensitive). */
export const matches = (c: Command, q: string) => q.toLowerCase().split(/\s+/).filter(Boolean).every((w) => `${c.group} ${c.label}`.toLowerCase().includes(w));

export default function CommandMenu({ commands, onClose }: { commands: Command[]; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [i, setI] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const shown = commands.filter((c) => matches(c, q)).slice(0, 50);
  const at = Math.min(i, Math.max(shown.length - 1, 0));
  useEffect(() => { list.current?.children[at]?.scrollIntoView({ block: "nearest" }); }, [at]);
  const run = (c?: Command) => { if (!c) return; onClose(); c.run(); };
  const key = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) { e.preventDefault(); setI((at + 1) % Math.max(shown.length, 1)); }
    else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) { e.preventDefault(); setI((at - 1 + shown.length) % Math.max(shown.length, 1)); }
    else if (e.key === "Enter") { e.preventDefault(); run(shown[at]); }
    else if (e.key === "Escape") { e.preventDefault(); onClose(); }
  };
  return (
    <div className="cmdk-back" onMouseDown={onClose}>
      <div className="cmdk" role="dialog" aria-label="Command menu" onMouseDown={(e) => e.stopPropagation()}>
        <input autoFocus placeholder="Type a command or search…" value={q} onChange={(e) => { setQ(e.target.value); setI(0); }} onKeyDown={key}
          role="combobox" aria-expanded aria-controls="cmdk-list" aria-activedescendant={shown[at] ? `cmdk-${at}` : undefined} />
        <ul id="cmdk-list" ref={list} role="listbox">
          {shown.map((c, n) => (
            <li key={c.id} id={`cmdk-${n}`} role="option" aria-selected={n === at} className={n === at ? "on" : ""} onMouseMove={() => setI(n)} onClick={() => run(c)}>
              <span className="cmdk-group">{c.group}</span><span className="cmdk-label">{c.label}</span>{c.hint && <kbd>{c.hint}</kbd>}
            </li>
          ))}
          {!shown.length && <li className="empty">No matches</li>}
        </ul>
      </div>
    </div>
  );
}
