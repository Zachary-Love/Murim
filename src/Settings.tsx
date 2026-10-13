import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { MODE_LABEL, AgentKind, ReplyMode } from "./types";

type S = { agent: AgentKind; agent_command: string; repos_root: string; default_permission_mode: "normal" | "allowlist" | "skip"; caffeine: boolean; default_prompt: string; reply_mode: ReplyMode; review_badge: "count" | "dot" | "off" };

export default function Settings() {
  const [s, setS] = useState<S | null>(null);
  const [key, setKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [msg, setMsg] = useState("");

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
    <form className="settings" onSubmit={(e) => { e.preventDefault(); save(); }}>
      <h2>Settings</h2>
      <label>Repos folder
        <div className="group">
          <input className="grow" value={s.repos_root} placeholder="Folder that contains your git repos" onChange={(e) => setS({ ...s, repos_root: e.target.value })} />
          <button type="button" className="btn" onClick={async () => {
            const dir = await open({ directory: true, defaultPath: s.repos_root || undefined, title: "Choose your repos folder" });
            if (typeof dir === "string") setS({ ...s, repos_root: dir });
          }}>Choose…</button>
        </div>
      </label>
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
      <label>Default prompt
        <textarea value={s.default_prompt} placeholder="Appended to every task prompt, e.g. run the tests before finishing" onChange={(e) => setS({ ...s, default_prompt: e.target.value })} />
      </label>
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
      <label>Review replies (Tempering)
        <select value={s.reply_mode} onChange={(e) => setS({ ...s, reply_mode: e.target.value as ReplyMode })}>
          <option value="off">Off</option>
          <option value="terse">Terse</option>
          <option value="full">Full</option>
        </select>
      </label>
      </>}
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
      <label>Linear API key {hasKey && <span className="chip green">saved</span>}
        <input type="password" value={key} placeholder={hasKey ? "enter to replace" : ""}
          onChange={(e) => setKey(e.target.value)} />
      </label>
      <div className="group"><button className="btn primary" type="submit">Save</button> <span className="muted">{msg}</span></div>
    </form>
  );
}
