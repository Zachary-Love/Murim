import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Automation, AutoSel, Schedule, Session, DAYS, runStatus } from "./types";
import { SessionPane } from "./MeditationPane";
import logo from "./assets/logo-mark.svg";

export const when = (secs: number) => new Date(secs * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
const today = () => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };
export const draft = (): Automation => ({ id: "", name: "New automation", prompt: "", folder: "", enabled: false, nextDue: null, runs: [],
  schedule: { kind: "weekly", time: "09:00", days: [1], start: today(), dayOfMonth: 1 } });
const STATUS = { running: "Running", waiting: "Waiting on you", done: "Done", failed: "Failed", missed: "Missed", skipped: "Skipped" };

type Props = { auto: Automation; sessions: Record<string, Session>; onOpen: (s: AutoSel | null) => void; onSaved: (a: Automation) => void };

export default function AutomationPane({ auto, sessions, onOpen, onSaved }: Props) {
  const [f, setF] = useState(auto); // runs etc. come from `auto`; only the form fields are local
  const [msg, setMsg] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [local, setLocal] = useState<string[]>([]);
  useEffect(() => {
    invoke<{ pavilion: { ollama_url: string } }>("get_settings").then((c) => invoke<string[]>("ollama_models", { url: c.pavilion.ollama_url })).then(setLocal).catch(() => {});
  }, []);
  const sch = (s: Partial<Schedule>) => setF({ ...f, schedule: { ...f.schedule, ...s } });
  const fail = (e: unknown) => setMsg(String(e));
  const save = () => invoke<Automation>("save_automation", { automation: { ...auto, ...f, runs: auto.runs } })
    .then((a) => { setMsg("Saved"); onSaved(a); }).catch(fail);
  const runNow = () => invoke<string>("run_automation_now", { id: auto.id })
    .then((r) => { setMsg(""); onOpen({ kind: "run", id: `automation:${auto.id}:${r}` }); }).catch(fail);
  const del = () => invoke("delete_automation", { id: auto.id }).then(() => onOpen(null)).catch(fail);
  const s = f.schedule, weekly = s.kind === "weekly" || s.kind === "biweekly";
  return (
    <div className="pane">
      <header className="topbar" data-tauri-drag-region>
        <span className="pane-title" data-tauri-drag-region>{auto.id ? auto.name : "New automation"}</span>
        <div className="topbar-right" data-tauri-drag-region>
          {auto.id && <button className="btn" onClick={runNow}>▶ Run now</button>}
          {auto.id && (confirm
            ? <><span className="muted">Delete?</span><button className="btn danger" onClick={del}>Yes</button><button className="btn" onClick={() => setConfirm(false)}>No</button></>
            : <button className="btn danger" onClick={() => setConfirm(true)}>Delete</button>)}
        </div>
      </header>
      <div className="auto-edit">
        <form className="settings" onSubmit={(e) => { e.preventDefault(); save(); }}>
          <label>Name<input value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} /></label>
          <label>Prompt<textarea value={f.prompt} placeholder="What Claude should do on each run" onChange={(e) => setF({ ...f, prompt: e.target.value })} /></label>
          <label>Folder
            <div className="group">
              <input className="grow" value={f.folder} placeholder="Repos folder" onChange={(e) => setF({ ...f, folder: e.target.value })} />
              <button type="button" className="btn" onClick={async () => {
                const dir = await open({ directory: true, defaultPath: f.folder || undefined, title: "Choose the folder to run in" });
                if (typeof dir === "string") setF({ ...f, folder: dir });
              }}>Choose…</button>
            </div>
          </label>
          <label>Model
            <select value={f.model ?? ""} onChange={(e) => setF({ ...f, model: e.target.value })}>
              <option value="">Claude default</option>
              <optgroup label="Claude"><option value="opus">Opus</option><option value="sonnet">Sonnet</option><option value="haiku">Haiku</option></optgroup>
              {(local.length > 0 || f.model?.startsWith("ollama:")) && <optgroup label="Local (Ollama)">
                {[...new Set([...local, ...(f.model?.startsWith("ollama:") ? [f.model.slice(7)] : [])])].map((m) => <option key={m} value={`ollama:${m}`}>{m}</option>)}
              </optgroup>}
            </select>
          </label>
          <label>Schedule
            <div className="group">
              <select value={s.kind} onChange={(e) => sch({ kind: e.target.value as Schedule["kind"] })}>
                <option value="daily">Daily</option>
                <option value="weekdays">Weekdays</option>
                <option value="weekly">Weekly</option>
                <option value="biweekly">Every 2 weeks</option>
                <option value="monthly">Monthly</option>
              </select>
              <span className="muted">at</span>
              <TimePick value={s.time} onChange={(time) => sch({ time })} />
            </div>
          </label>
          {weekly && <div className="group days">
            {DAYS.map((d, i) => (
              <button type="button" key={i} title={d} className={`btn toggle ${s.days.includes(i) ? "on" : ""}`}
                onClick={() => sch({ days: s.days.includes(i) ? s.days.filter((x) => x !== i) : [...s.days, i].sort() })}>{d[0]}</button>
            ))}
          </div>}
          {s.kind === "biweekly" && <label>Starting<input type="date" value={s.start} onChange={(e) => sch({ start: e.target.value })} /></label>}
          {s.kind === "monthly" && <label>Day of month<input type="number" min={1} max={31} value={s.dayOfMonth} onChange={(e) => sch({ dayOfMonth: +e.target.value })} /></label>}
          <label className="check"><input type="checkbox" checked={f.enabled} onChange={(e) => setF({ ...f, enabled: e.target.checked })} />Enabled</label>
          <div className="group"><button className="btn primary" type="submit">Save</button> <span className={msg && msg !== "Saved" ? "err" : "muted"}>{msg}</span></div>
        </form>
        {auto.id && <>
          <div className="section-label runs-label">Runs</div>
          <ul className="tasks meds runs">
            {auto.runs.map((r) => {
              const st = runStatus(r, sessions);
              return (
                <li key={r.id}>
                  <button className={`row ${st === "waiting" ? "attn" : ""}`} disabled={!r.sessionId} title={r.error ?? undefined}
                    onClick={() => r.sessionId && onOpen({ kind: "run", id: r.sessionId })}>
                    <span className="t">{when(r.started)}</span>
                    {st === "running" && <i className="ico spin" />}
                    {st === "waiting" && <i className="ico dot-input" />}
                    <span className={`st ${st}`}>{STATUS[st]}</span>
                  </button>
                  <button className="x" title="Delete this run" onClick={() => invoke("delete_run", { autoId: auto.id, runId: r.id }).catch(fail)}>×</button>
                </li>
              );
            })}
            {!auto.runs.length && <li className="empty">No runs yet.</li>}
          </ul>
        </>}
      </div>
    </div>
  );
}

/** 12-hour hour / minute / AM-PM selects over a 24h "HH:MM" value. */
function TimePick({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [h24, m] = value.split(":").map(Number);
  const pm = h24 >= 12, h12 = h24 % 12 || 12;
  const set = (h: number, min: number, p: boolean) => onChange(`${String((h % 12) + (p ? 12 : 0)).padStart(2, "0")}:${String(min).padStart(2, "0")}`);
  const mins = [...new Set([...Array.from({ length: 12 }, (_, i) => i * 5), m])].sort((a, b) => a - b);
  return <>
    <select aria-label="Hour" value={h12} onChange={(e) => set(+e.target.value, m, pm)}>{Array.from({ length: 12 }, (_, i) => i + 1).map((h) => <option key={h} value={h}>{h}</option>)}</select>
    <select aria-label="Minute" value={m} onChange={(e) => set(h12, +e.target.value, pm)}>{mins.map((x) => <option key={x} value={x}>{String(x).padStart(2, "0")}</option>)}</select>
    <select aria-label="AM or PM" value={pm ? "pm" : "am"} onChange={(e) => set(h12, m, e.target.value === "pm")}><option value="am">AM</option><option value="pm">PM</option></select>
  </>;
}

export function RunPane({ auto, id, session }: { auto?: Automation; id: string; session?: Session }) {
  const run = auto?.runs.find((r) => r.sessionId === id);
  const title = `${auto?.name ?? "Automation"}${run ? ` · ${when(run.started)}` : ""}`;
  return session ? <SessionPane key={id} id={id} session={session} title={title} noun="run" />
    : <div className="empty-state"><img className="logo-lg" src={logo} alt="" /><p>This run's session is gone.</p></div>;
}
