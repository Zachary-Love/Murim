import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
// iTerm2's default palette, so CLI colors look like the user's own terminal.
const theme = {
  background: "#0a0a0a", foreground: "#e5e5e5", cursor: "#f43f5e", cursorAccent: "#0a0a0a", selectionBackground: "#f43f5e44",
  black: "#000000", red: "#c91b00", green: "#00c200", yellow: "#c7c400", blue: "#0225c7", magenta: "#c930c7", cyan: "#00c5c7", white: "#c7c7c7",
  brightBlack: "#686868", brightRed: "#ff6e67", brightGreen: "#5ffa68", brightYellow: "#fffc67", brightBlue: "#6871ff", brightMagenta: "#ff77ff", brightCyan: "#60fdff", brightWhite: "#ffffff",
};

export default function Terminal({ issueId }: { issueId: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current!;
    const fontFamily = getComputedStyle(el).getPropertyValue("--mono");
    const term = new XTerm({ fontSize: 12.5, fontFamily, cursorBlink: true, theme });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const resize = () => { if (!term.element) return; fit.fit(); invoke("pty_resize", { issueId, cols: term.cols, rows: term.rows }).catch(() => {}); };
    const ro = new ResizeObserver(resize);
    // xterm measures cells at open(): open only once the bundled font is loaded, or it sizes against the fallback.
    let dead = false;
    document.fonts.load(`12.5px ${fontFamily}`).catch(() => {}).finally(() => { if (!dead) { term.open(el); ro.observe(el); } });
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
    return () => { dead = true; ro.disconnect(); d.dispose(); un.then((f) => f()); term.dispose(); };
  }, [issueId]);
  return <div className="term" ref={ref} />;
}
