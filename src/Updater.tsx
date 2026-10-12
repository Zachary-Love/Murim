import { useEffect, useState } from "react";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export default function Updater() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [status, setStatus] = useState("");
  useEffect(() => {
    // Silent on failure: offline, or no latest.json published yet.
    const poll = () => check().then(setUpdate).catch(() => {});
    poll();
    const t = setInterval(poll, 4 * 60 * 60 * 1000);
    return () => clearInterval(t);
  }, []);
  if (!update) return null;
  const install = async () => {
    let total = 0, got = 0;
    setStatus("Downloading…");
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === "Started") total = e.data.contentLength ?? 0;
        else if (e.event === "Progress") { got += e.data.chunkLength; if (total) setStatus(`Downloading ${Math.round((got / total) * 100)}%`); }
        else setStatus("Installing…");
      });
      await relaunch();
    } catch (e) {
      setStatus(`Update failed: ${e}`);
    }
  };
  return (
    <button className="btn ghost" disabled={!!status && !status.startsWith("Update failed")} title={status} onClick={install}>
      {status || `Update to v${update.version}`}
    </button>
  );
}
