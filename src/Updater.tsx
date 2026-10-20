import { useEffect, useState } from "react";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { DownloadIcon } from "./Icons";

export default function Updater() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [status, setStatus] = useState("");
  useEffect(() => {
    // Silent on failure: offline, or no latest.json published yet. Polls every 30 min and on focus, one check at a time.
    let busy = false;
    const poll = () => { if (busy) return; busy = true; check().then(setUpdate).catch(() => {}).finally(() => { busy = false; }); };
    poll();
    const t = setInterval(poll, 30 * 60 * 1000);
    window.addEventListener("focus", poll);
    return () => { clearInterval(t); window.removeEventListener("focus", poll); };
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
    <button className="btn icon update" disabled={!!status && !status.startsWith("Update failed")} title={status || `Update to v${update.version}`} onClick={install}>
      <DownloadIcon />
    </button>
  );
}
