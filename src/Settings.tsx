import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import AppearanceSettings from "./AppearanceSettings";
import { MODE_LABEL, AgentKind, ReplyMode } from "./types";

type S = { agent: AgentKind; agent_command: string; repos_root: string; meditations_dir: string; default_permission_mode: "normal" | "allowlist" | "skip"; caffeine: boolean; default_prompt: string; pr_prompt: string; bedrock: boolean; aws_region: string; aws_profile: string; review_prompt: string; review_blocklist: string; reply_mode: ReplyMode; review_badge: "count" | "dot" | "off"; pavilion: { local_model: boolean; ollama_url: string; model: string; max_agents: number; docker_slots: number; auto_temper: boolean; auto_investigate: boolean; auto_review: boolean; plan_first: boolean; claude_check: "always" | "lowConfidence" | "off"; auto_approve_at: number } };

export const SECTIONS = ["Appearance", "Prompts", "Folders", "AI", "Pavilion", "Orchestrator Log", "Accounts"] as const;
export type Section = typeof SECTIONS[number];

export function SettingsNav({ tab, onTab }: { tab: Section; onTab: (t: Section) => void }) {
  return (
    <>
      <div className="section-label">Settings</div>
      <ul className="tasks">
        {SECTIONS.map((t) => <li key={t}><button className={`row ${tab === t ? "on" : ""}`} onClick={() => onTab(t)}><span className="t">{t}</span></button></li>)}
      </ul>
    </>
  );
}

const MEMORY_MAX = 2000; // judge::MEMORY_MAX

/** ~/.murim/pavilion-memory.md: lessons the orchestrator rewrites when corrected; editable here. */
function OrchestratorMemory() {
  const [text, setText] = useState<string | null>(null); // null = following the file
  const [disk, setDisk] = useState("");
  const [msg, setMsg] = useState("");
  useEffect(() => {
    const load = () => invoke<string>("get_pavilion_memory").then(setDisk).catch(() => {});
    load();
    const t = setInterval(load, 5000);
    return () => clearInterval(t);
  }, []);
  const v = text ?? disk, n = v.trim().length;
  const save = () => invoke("set_pavilion_memory", { text: v }).then(() => { setDisk(v.trim()); setText(null); setMsg("Saved"); }).catch((e) => setMsg(String(e)));
  return <>
    <label>Memory <span className={`muted ${n > MEMORY_MAX ? "warn" : ""}`}>{n} / {MEMORY_MAX}</span>
      <textarea value={v} placeholder="Empty. Pavilion adds lessons when you correct it (move a card, investigate anyway, dismiss a blocker) or when Claude overrules the local model." onChange={(e) => { setText(e.target.value); setMsg(""); }} />
    </label>
    <div className="group">
      <button className="btn" type="button" disabled={text === null || n > MEMORY_MAX} onClick={save}>Save memory</button>
      {text !== null && <button className="btn ghost" type="button" onClick={() => { setText(null); setMsg(""); }}>Discard</button>}
      <span className="muted">{msg}</span>
    </div>
    <p className="muted">Added to the top of every judgement prompt. Pavilion rewrites it itself and can never make it longer than {MEMORY_MAX} characters.</p>
  </>;
}

/** ~/.murim/orchestrator.log, polled while open; stays pinned to the bottom unless you scroll up. */
function OrchestratorLog() {
  const [text, setText] = useState("");
  const pre = useRef<HTMLPreElement>(null);
  useEffect(() => {
    const load = () => invoke<string>("orchestrator_log").then((t) => {
      const el = pre.current, pinned = !el || el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      setText(t);
      if (pinned) requestAnimationFrame(() => { if (pre.current) pre.current.scrollTop = pre.current.scrollHeight; });
    }).catch((e) => setText(String(e)));
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, []);
  return <>
    <OrchestratorMemory />
    <p className="muted">What Pavilion's local model and Claude decided, and why Pavilion did or didn't start agents. Also in <code>~/.murim/orchestrator.log</code>.</p>
    <pre ref={pre} className="olog">{text || "Nothing logged yet. Entries appear on the next board check (every minute) or judgement."}</pre>
  </>;
}

// Stays mounted across sections, so unsaved edits survive switching tabs; one Save covers them all.
export default function Settings({ tab }: { tab: Section }) {
  const [s, setS] = useState<S | null>(null);
  const [key, setKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [msg, setMsg] = useState("");
  const [test, setTest] = useState("");
  const [models, setModels] = useState<string[] | null>(null); // installed in Ollama; null = couldn't list
  const [mErr, setMErr] = useState("");
  const loadModels = (url: string) => invoke<string[]>("ollama_models", { url }).then((m) => { setModels(m); setMErr(""); }).catch((e) => { setModels(null); setMErr(String(e)); });
  const ready = !!s;
  useEffect(() => { if (ready && tab === "Pavilion") loadModels(s!.pavilion.ollama_url); }, [ready, tab]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    invoke<S>("get_settings").then(setS);
    invoke<boolean>("has_linear_key").then(setHasKey);
  }, []);
  if (!s) return null;

  const save = async () => {
    try {
      await invoke("save_settings", { settings: s });
      if (key) {
        await invoke("set_linear_key", { key });
        setKey("");
        setHasKey(await invoke<boolean>("has_linear_key"));
      }
      invoke("refresh_inbox").catch(() => {}); // pick up a new repos folder / key now, not at the next poll
      setMsg("Saved");
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <form className="pane settings-page" onSubmit={(e) => { e.preventDefault(); save(); }}>
      <header className="topbar" data-tauri-drag-region>
        <h2 data-tauri-drag-region>{tab}</h2>
        <div className="topbar-right"><span className="muted">{msg}</span>{tab !== "Appearance" && tab !== "Orchestrator Log" && <button className="btn primary" type="submit">Save</button>}</div>
      </header>
      <div className="settings">
      {tab === "Folders" && <>
      <label>Repos folder
        <div className="group">
          <input className="grow" value={s.repos_root} placeholder="Folder that contains your git repos" onChange={(e) => setS({ ...s, repos_root: e.target.value })} />
          <button type="button" className="btn" onClick={async () => {
            const dir = await open({ directory: true, defaultPath: s.repos_root || undefined, title: "Choose your repos folder" });
            if (typeof dir === "string") setS({ ...s, repos_root: dir });
          }}>Choose…</button>
        </div>
      </label>
      <label>Meditations folder
        <div className="group">
          <input className="grow" value={s.meditations_dir ?? ""} placeholder="~/.murim/meditations" onChange={(e) => setS({ ...s, meditations_dir: e.target.value })} />
          <button type="button" className="btn" onClick={async () => {
            const dir = await open({ directory: true, defaultPath: s.meditations_dir || undefined, title: "Choose your meditations folder" });
            if (typeof dir === "string") setS({ ...s, meditations_dir: dir });
          }}>Choose…</button>
        </div>
      </label>
      </>}
      {tab === "AI" && <>
      <label>Agent
        <select value={s.agent} onChange={(e) => setS({ ...s, agent: e.target.value as AgentKind })}>
          <option value="claude">Claude Code (full automation)</option>
          <option value="custom">Custom command</option>
        </select>
      </label>
      {s.agent === "custom" && (
        <>
          <label>Command
            <input className="mono" value={s.agent_command} placeholder="e.g. codex, gemini, aider --yes" onChange={(e) => setS({ ...s, agent_command: e.target.value })} />
          </label>
          <p className="muted">Runs in the task's worktree through your login shell, with the issue as the last argument.
            Custom agents get the terminal, extra terminals, the Review tab and Accept PR, but not question relay,
            the automatic quality pass and draft PR, or Tempering — those rely on Claude Code's hooks.</p>
        </>
      )}
      {s.agent === "claude" && <>
      <label>Default permission mode
        <select value={s.default_permission_mode}
          onChange={(e) => setS({ ...s, default_permission_mode: e.target.value as S["default_permission_mode"] })}>
          {(Object.keys(MODE_LABEL) as S["default_permission_mode"][]).map((m) => <option key={m} value={m}>{MODE_LABEL[m]}</option>)}
        </select>
      </label>
      {s.default_permission_mode === "skip" && (
        <p className="warn">Bypass permissions runs Claude Code with --dangerously-skip-permissions: no prompts for any tool.</p>
      )}
      <label>Claude provider
        <select value={s.bedrock ? "bedrock" : "anthropic"} onChange={(e) => setS({ ...s, bedrock: e.target.value === "bedrock" })}>
          <option value="anthropic">Anthropic (your Claude login)</option>
          <option value="bedrock">Amazon Bedrock</option>
        </select>
      </label>
      {s.bedrock && <>
        <label>AWS region
          <input className="mono" value={s.aws_region} placeholder="us-east-1 (blank: your AWS config)" onChange={(e) => setS({ ...s, aws_region: e.target.value })} />
        </label>
        <label>AWS profile
          <input className="mono" value={s.aws_profile} placeholder="blank: default credentials" onChange={(e) => setS({ ...s, aws_profile: e.target.value })} />
        </label>
        <p className="muted">Every Claude Code agent and judgement call runs with CLAUDE_CODE_USE_BEDROCK=1 and these AWS settings.
          Claude.ai connectors don't load on Bedrock, so agents get Linear's MCP server with your Linear API key instead.</p>
      </>}
      <label>Review replies (Tempering)
        <select value={s.reply_mode} onChange={(e) => setS({ ...s, reply_mode: e.target.value as ReplyMode })}>
          <option value="off">Off</option>
          <option value="terse">Terse</option>
          <option value="full">Full</option>
        </select>
      </label>
      </>}
      <label>Max agents at once
        <input type="number" min={1} max={50} value={s.pavilion.max_agents} onChange={(e) => setS({ ...s, pavilion: { ...s.pavilion, max_agents: Math.max(1, Number(e.target.value) || 1) } })} />
      </label>
      <p className="muted">How many agents Pavilion keeps working before it stops starting new investigations. Agents you start yourself aren't limited.</p>
      <label>Docker jobs at once
        <input type="number" min={0} max={20} value={s.pavilion.docker_slots} onChange={(e) => setS({ ...s, pavilion: { ...s.pavilion, docker_slots: Math.max(0, Math.min(20, Number(e.target.value) || 0)) } })} />
      </label>
      <p className="muted">Docker builds, runs and compose ups that agents can have going at once; the rest wait their turn so the Mac stays responsive. 0 turns the queue off. Applies to running agents too.</p>
      </>}
      {tab === "Pavilion" && (() => {
        const pv = s.pavilion, set = (p: Partial<S["pavilion"]>) => setS({ ...s, pavilion: { ...pv, ...p } });
        return <>
      <label>Judgement model
        <select value={pv.local_model ? "local" : "haiku"} onChange={(e) => set({ local_model: e.target.value === "local" })}>
          <option value="local">Local model (Ollama)</option>
          <option value="haiku">Claude Haiku</option>
        </select>
      </label>
      <p className="muted">Who triages issues and review comments and keeps Pavilion's memory. Claude Haiku needs no setup but every judgement is a Claude call.</p>
      {pv.local_model && <>
      <label>Ollama URL
        <input className="mono" value={pv.ollama_url} onChange={(e) => set({ ollama_url: e.target.value })} onBlur={() => loadModels(pv.ollama_url)} />
      </label>
      <label>Local model
        <div className="group">
          {models?.length ? (
            <select className="mono" value={pv.model} onChange={(e) => { set({ model: e.target.value }); setTest(""); }}>
              {!models.includes(pv.model) && <option value={pv.model}>{pv.model || "Choose a model"} (not installed)</option>}
              {models.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
          ) : <input className="mono" value={pv.model} placeholder="qwen3:4b" onChange={(e) => set({ model: e.target.value })} />}
          <button className="btn" type="button" title="Reload installed models" onClick={() => loadModels(pv.ollama_url)}>Refresh</button>
          <button className="btn" type="button" onClick={() => { setTest("Testing…"); invoke<string>("pavilion_test_model", { url: pv.ollama_url, model: pv.model }).then(setTest).catch((e) => setTest(String(e))); }}>Test</button>
        </div>
      </label>
      {test && <p className="muted">{test}</p>}
      {mErr ? <p className="muted">{mErr}</p> : models && !models.length && <p className="muted">Ollama is running but has no models yet.</p>}
      <p className="muted">Install Ollama and run <code>ollama pull {pv.model || "qwen3:4b"}</code>. Without it, Pavilion's judgement calls go to Claude.</p>
      </>}
      <label>Start ready issues automatically
        <select value={pv.auto_investigate ? "on" : "off"} onChange={(e) => set({ auto_investigate: e.target.value === "on" })}>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </label>
      <label>Review teammates' PRs automatically
        <select value={pv.auto_review ? "on" : "off"} onChange={(e) => set({ auto_review: e.target.value === "on" })}>
          <option value="on">On, once CI passes and no person has reviewed</option>
          <option value="off">Off, they show up in Needs you for you to review</option>
        </select>
      </label>
      <label>Never review PRs by
        <input value={s.review_blocklist} placeholder="GitHub usernames, comma separated, e.g. Zachary-Love, dependabot" onChange={(e) => setS({ ...s, review_blocklist: e.target.value })} />
      </label>
      <label>Plan before coding
        <select value={pv.plan_first ? "on" : "off"} onChange={(e) => set({ plan_first: e.target.value === "on" })}>
          <option value="off">Off, ready issues go straight to the coding agent</option>
          <option value="on">On, investigate and write a plan first</option>
        </select>
      </label>
      {pv.plan_first && <label>Approve plans automatically
        <select value={String(pv.auto_approve_at)} onChange={(e) => set({ auto_approve_at: Number(e.target.value) })}>
          <option value="0">Never, always ask me</option>
          <option value="0.6">When 60% sure</option>
          <option value="0.75">When 75% sure</option>
          <option value="0.9">When 90% sure</option>
        </select>
      </label>}
      <p className="muted">When an investigation finishes, the orchestrator reviews the plan. If it's sure enough the plan is concrete and low-risk, coding starts without you; risky plans (migrations, deletions, security, infrastructure, public APIs) and open questions come to you.</p>
      <label>Handle PR feedback automatically
        <select value={pv.auto_temper ? "on" : "off"} onChange={(e) => set({ auto_temper: e.target.value === "on" })}>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </label>
      <p className="muted">Turns Tempering on for every PR an agent opens: failing checks, merge conflicts and review feedback go back to the agent. The local model sorts each review comment first: changes go to the agent, questions and decisions come to you, praise and optional nits are skipped. Switching Tempering off on a task keeps it off.</p>
      {pv.local_model && <>
      <label>Claude check
        <select value={pv.claude_check} onChange={(e) => set({ claude_check: e.target.value as S["pavilion"]["claude_check"] })}>
          <option value="always">Always</option>
          <option value="lowConfidence">Low confidence only</option>
          <option value="off">Off</option>
        </select>
      </label>
      <p className="muted">Claude (Haiku) checks the local model before Pavilion starts an investigation or marks an issue blocked.</p>
      </>}
      {pv.plan_first && <p className="muted">Investigations can't edit files and run in your default permission mode; coding starts when the plan is approved.</p>}
        </>;
      })()}
      {tab === "Orchestrator Log" && <OrchestratorLog />}
      {tab === "Prompts" && (
      <label>Default prompt
        <textarea value={s.default_prompt} placeholder="Appended to the first prompt of every task, e.g. coding conventions or run the tests before finishing" onChange={(e) => setS({ ...s, default_prompt: e.target.value })} />
      </label>
      )}
      {tab === "Prompts" && <>
      <label>Pull request prompt
        <textarea value={s.pr_prompt} placeholder="Followed right before the agent opens a draft PR, e.g. record proof with proofshot and add it to the PR description" onChange={(e) => setS({ ...s, pr_prompt: e.target.value })} />
      </label>
      <label>Review prompt
        <textarea value={s.review_prompt} placeholder="Appended to every review agent's prompt, e.g. focus on security and tests" onChange={(e) => setS({ ...s, review_prompt: e.target.value })} />
      </label>
</>}
      {tab === "Appearance" && <AppearanceSettings />}
      {tab === "Appearance" && (
      <label>Review requests badge
        <select value={s.review_badge} onChange={(e) => {
          const badge = e.target.value as S["review_badge"];
          setS({ ...s, review_badge: badge });
          invoke("set_review_badge", { badge }).then(() => window.dispatchEvent(new CustomEvent("murim-badge", { detail: badge }))).catch((err) => setMsg(String(err)));
        }}>
          <option value="count">Number</option>
          <option value="dot">Dot</option>
          <option value="off">Off</option>
        </select>
      </label>
      )}
      {tab === "Accounts" && (
      <label>Linear API key {hasKey && <span className="chip green">saved</span>}
        <input type="password" value={key} placeholder={hasKey ? "enter to replace" : ""}
          onChange={(e) => setKey(e.target.value)} />
      </label>
      )}
      </div>
    </form>
  );
}
