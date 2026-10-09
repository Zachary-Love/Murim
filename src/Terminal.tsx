import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

export default function Terminal({ issueId }: { issueId: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const term = new XTerm({ fontSize: 13, cursorBlink: true, theme: { background: "#0e0f11", foreground: "#d4d6da" } });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(ref.current!);
    const resize = () => { fit.fit(); invoke("pty_resize", { issueId, cols: term.cols, rows: term.rows }).catch(() => {}); };
    const ro = new ResizeObserver(resize);
    ro.observe(ref.current!);
    // Buffer live output until the snapshot is written so ordering holds (may duplicate a chunk at the seam).
    let ready = false;
    const pending: Uint8Array[] = [];
    const un = listen<{ issueId: string; data: string }>("pty-output", (e) => {
      if (e.payload.issueId !== issueId) return;
      const b = bytes(e.payload.data);
      ready ? term.write(b) : pending.push(b);
    });
    invoke<string>("pty_snapshot", { issueId })
      .then((s) => term.write(bytes(s)))
      .catch(() => {})
      .finally(() => { ready = true; pending.forEach((b) => term.write(b)); resize(); });
    const d = term.onData((data) => invoke("pty_write", { issueId, data }).catch(() => {}));
    return () => { ro.disconnect(); d.dispose(); un.then((f) => f()); term.dispose(); };
  }, [issueId]);
  return <div className="term" ref={ref} />;
}
