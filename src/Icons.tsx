import type { Status } from "./types";
import logo from "./assets/logo-mark.svg";

/** The logo blade slashing in, on a loop: Murim's loading state. */
export const Loading = ({ label, small }: { label: string; small?: boolean }) => (
  <div className={`loading ${small ? "small" : ""}`}><img className="slash" src={logo} alt="" /><span>{label}</span></div>
);

const S = { width: 14, height: 14, viewBox: "0 0 14 14", fill: "none", strokeWidth: 1.5 } as const;

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

export const ReviewsIcon = () => (
  <svg {...L}><circle cx="6" cy="6" r="2.5" /><circle cx="6" cy="18" r="2.5" /><circle cx="18" cy="18" r="2.5" /><path d="M6 8.5v7M18 15.5V9a3 3 0 0 0-3-3h-3" /><path d="m14 4-2 2 2 2" /></svg>
);

export const MeditationsIcon = () => (
  <svg {...L}><path d="M12 20c-4-2-6-5-6-9 3 0 5 2 6 4 1-2 3-4 6-4 0 4-2 7-6 9Z" /><path d="M12 15c-1.5-2-1.5-6 0-10 1.5 4 1.5 8 0 10" /><path d="M3 13c0 4 4 7 9 7s9-3 9-7" /></svg>
);

export const DownloadIcon = () => (
  <svg {...L}><path d="M12 3v12M7 10l5 5 5-5" /><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" /></svg>
);

export const ClockIcon = () => (
  <svg {...L}><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></svg>
);

export const PlusIcon = () => (
  <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><path d="M6 1.5v9M1.5 6h9" /></svg>
);

export const RobotIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="5" y="8" width="14" height="11" rx="2" /><path d="M12 4v4M9 13v1M15 13v1" /></svg>
);

export const RefreshIcon = () => (
  <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M10.2 6.6A4.2 4.2 0 1 1 9 3" /><path d="M9.4 1v2.4H7" /></svg>
);

export const PavilionIcon = () => (
  <svg {...L}><path d="M2 9 12 3l10 6" /><path d="M4 9h16" /><path d="M6 9v10M18 9v10M10 9v10M14 9v10" /><path d="M3 21h18" /></svg>
);
