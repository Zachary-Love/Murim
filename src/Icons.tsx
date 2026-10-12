import { STAGE_LABEL, type Stage, type Status } from "./types";

const S = { width: 14, height: 14, viewBox: "0 0 14 14", fill: "none", strokeWidth: 1.5 } as const;

// Linear-style stage icon: ring for todo, filling pie as the task advances, check when done.
export const StateIcon = ({ stage, backlog }: { stage: Stage; backlog?: boolean }) => {
  const pie = (f: number) => {
    const a = 2 * Math.PI * f, x = 7 + 3.5 * Math.sin(a), y = 7 - 3.5 * Math.cos(a);
    return `M7 7V3.5A3.5 3.5 0 ${f > 0.5 ? 1 : 0} 1 ${x.toFixed(2)} ${y.toFixed(2)}z`;
  };
  return (
    <svg {...S} className={`ico stage ${stage}`} stroke="currentColor">
      <title>{STAGE_LABEL[stage]}</title>
      {stage === "done" ? (
        <><circle cx="7" cy="7" r="6" fill="currentColor" stroke="none" /><path d="M4.5 7.2l1.7 1.7 3.4-3.6" stroke="var(--bg)" strokeWidth="1.6" /></>
      ) : (
        <>
          <circle cx="7" cy="7" r="5.5" strokeDasharray={backlog && stage === "todo" ? "2 2" : undefined} />
          {stage === "progress" && <path d={pie(0.5)} fill="currentColor" stroke="none" />}
          {stage === "needsReview" && <path d={pie(0.75)} fill="currentColor" stroke="none" />}
          {stage === "codeReview" && <path d={pie(0.9)} fill="currentColor" stroke="none" />}
        </>
      )}
    </svg>
  );
};

export const AgentIcon = ({ status }: { status?: Status }) => {
  if (status === "starting" || status === "working") return <i className="ico spin" title="Agent working" />;
  if (status === "needsInput") return <i className="ico dot-input" title="Agent needs input" />;
  if (status === "idle") return <i className="ico dot-idle" title="Agent idle" />;
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

const L = { width: 16, height: 16, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: 1.8, strokeLinecap: "round", strokeLinejoin: "round", className: "ico" } as const;

export const CoffeeIcon = () => (
  <svg {...L}><path d="M17 8h1a4 4 0 1 1 0 8h-1" /><path d="M3 8h14v9a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4Z" /><path d="M6 2v2M10 2v2M14 2v2" /></svg>
);

export const GearIcon = () => (
  <svg {...L}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" /></svg>
);

export const FlameIcon = () => (
  <svg {...L}><path d="M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.07-2.14-.22-4.05 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.15.43-2.29 1-3a2.5 2.5 0 0 0 2.5 2.5z" /></svg>
);

export const FolderIcon = () => (
  <svg {...L}><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" /></svg>
);

export const TasksIcon = () => (
  <svg {...L}><path d="M9 6h11M9 12h11M9 18h11" /><path d="m3.5 6 1 1 2-2M3.5 12l1 1 2-2M3.5 18l1 1 2-2" /></svg>
);

export const ReviewsIcon = () => (
  <svg {...L}><circle cx="6" cy="6" r="2.5" /><circle cx="6" cy="18" r="2.5" /><circle cx="18" cy="18" r="2.5" /><path d="M6 8.5v7M18 15.5V9a3 3 0 0 0-3-3h-3" /><path d="m14 4-2 2 2 2" /></svg>
);

export const DownloadIcon = () => (
  <svg {...L}><path d="M12 3v12M7 10l5 5 5-5" /><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" /></svg>
);
