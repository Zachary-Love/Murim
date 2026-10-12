import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MODE_LABEL } from "./types";

type S = { repos_root: string; default_permission_mode: "normal" | "allowlist" | "skip"; caffeine: boolean };

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
      setMsg("Saved");
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <form className="settings" onSubmit={(e) => { e.preventDefault(); save(); }}>
      <h2>Settings</h2>
      <label>Repos folder
        <input value={s.repos_root} onChange={(e) => setS({ ...s, repos_root: e.target.value })} />
      </label>
      <label>Default Claude permission mode
        <select value={s.default_permission_mode}
          onChange={(e) => setS({ ...s, default_permission_mode: e.target.value as S["default_permission_mode"] })}>
          {(Object.keys(MODE_LABEL) as S["default_permission_mode"][]).map((m) => <option key={m} value={m}>{MODE_LABEL[m]}</option>)}
        </select>
      </label>
      {s.default_permission_mode === "skip" && (
        <p className="warn">Bypass permissions runs Claude with --dangerously-skip-permissions: no prompts for any tool.</p>
      )}
      <label>Linear API key {hasKey && <span className="chip green">saved</span>}
        <input type="password" value={key} placeholder={hasKey ? "enter to replace" : ""}
          onChange={(e) => setKey(e.target.value)} />
      </label>
      <div className="group"><button className="btn primary" type="submit">Save</button> <span className="muted">{msg}</span></div>
    </form>
  );
}
