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
      <button className={`btn icon ${on ? "on" : ""}`} aria-pressed={on} aria-label="Caffeine" title={`Caffeine ${on ? "on" : "off"}: keep this Mac awake so Claude keeps running`} onClick={() => toggle(!on)}>☕</button>
      {err && <div className="err">{err}</div>}
    </>
  );
}
