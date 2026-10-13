import { PlusIcon } from "./Icons";
import { Automation, AutoSel, Session, scheduleText, until, runStatus } from "./types";

type Props = { autos: Automation[]; sessions: Record<string, Session>; selAuto: string | null; onOpen: (s: AutoSel) => void };

export default function AutomationList({ autos, sessions, selAuto, onOpen }: Props) {
  return (
    <>
      <div className="section-label">Automations
        <button className="btn ghost new" title="New automation" onClick={() => onOpen({ kind: "automation", id: "" })}><PlusIcon /></button>
      </div>
      <ul className="tasks autos">
        {autos.map((a) => {
          const waiting = a.runs.some((r) => runStatus(r, sessions) === "waiting");
          return (
            <li key={a.id}>
              <button className={`row ${selAuto === a.id ? "on" : ""} ${a.enabled ? "" : "off"} ${waiting ? "attn" : ""}`} title={a.name} onClick={() => onOpen({ kind: "automation", id: a.id })}>
                <span className="t">{a.name}</span>
                {waiting && <i className="ico dot-input" title="A run is waiting on you" />}
                <span className="ident">{a.enabled ? until(a.nextDue) : "—"}</span>
                <span className="sub">{scheduleText(a.schedule)}</span>
              </button>
            </li>
          );
        })}
        {!autos.length && <li className="empty">Nothing yet.</li>}
      </ul>
    </>
  );
}
