import type { Status } from "./types";

const S = { width: 14, height: 14, viewBox: "0 0 14 14", fill: "none", strokeWidth: 1.5 } as const;

// Mimics Linear's workflow-state icons.
export const StateIcon = ({ type }: { type: string }) => {
  const c = type === "started" ? "#f2c94c" : type === "triage" ? "#f2994a" : "#8a8f98";
  return (
    <svg {...S} stroke={c} className="ico">
      <circle cx="7" cy="7" r="5.5" strokeDasharray={type === "backlog" ? "2 2" : undefined} />
      {type === "started" && <path d="M7 3.5a3.5 3.5 0 0 1 0 7z" fill={c} stroke="none" />}
      {type === "triage" && <circle cx="7" cy="7" r="2" fill={c} stroke="none" />}
    </svg>
  );
};

// Claude's starburst mark, in the Claude accent.
export const ClaudeMark = ({ size = 14 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 14 14" className="ico claude-mark" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
    {[0, 45, 90, 135].map((a) => <line key={a} x1="7" y1="1.5" x2="7" y2="12.5" transform={`rotate(${a} 7 7)`} />)}
  </svg>
);

export const ClaudeIcon = ({ status }: { status?: Status }) => {
  if (status === "starting" || status === "working") return <i className="ico spin" title="Claude working" />;
  if (status === "needsInput") return <i className="ico dot-input" title="Claude needs input" />;
  if (status === "idle") return <i className="ico dot-idle" title="Claude idle" />;
  return null;
};

export const PrIcon = ({ color, n, title }: { color: string; n?: number; title: string }) => (
  <span className={`pri ${color}`} title={title}>
    <svg {...S} stroke="currentColor" className="ico">
      <circle cx="3.5" cy="3" r="1.5" /><circle cx="3.5" cy="11" r="1.5" /><circle cx="10.5" cy="11" r="1.5" />
      <path d="M3.5 4.5v5M10.5 9.5V6a2 2 0 0 0-2-2H7" />
    </svg>
    {!!n && <sub>{n}</sub>}
  </span>
);
