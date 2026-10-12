import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export default function Caffeine() {
  const [on, setOn] = useState(false);
  const [err, setErr] = useState("");
  useEffect(() => {
    invoke<boolean>("get_caffeine").then(setOn).catch(() => {});
  }, []);
  const toggle = (enabled: boolean) =>
    invoke("set_caffeine", { enabled })
      .then(() => { setOn(enabled); setErr(""); })
      .catch((e) => setErr(String(e)));
  return (
    <>
      <button className={`btn toggle ${on ? "on" : ""}`} aria-pressed={on} title="Keep this Mac awake so Claude keeps running" onClick={() => toggle(!on)}>
        ☕ Caffeine {on ? "on" : "off"}
      </button>
      {err && <div className="err">{err}</div>}
    </>
  );
}
