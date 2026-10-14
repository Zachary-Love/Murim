import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { Appearance, currentAppearance, family, isLight, loadFont } from "./appearance";

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
// "Match Murim theme" palettes (iTerm2's defaults for dark and light backgrounds); also the fallback without an iTerm profile.
const DARK = ["#000000", "#c91b00", "#00c200", "#c7c400", "#0225c7", "#c930c7", "#00c5c7", "#c7c7c7", "#686868", "#ff6e67", "#5ffa68", "#fffc67", "#6871ff", "#ff77ff", "#60fdff", "#ffffff"];
const LIGHT = ["#000000", "#c91b00", "#00a600", "#999900", "#0225c7", "#b200b2", "#00a6b2", "#bfbfbf", "#666666", "#e50000", "#00d900", "#e5e500", "#0000ff", "#e500e5", "#00e5e5", "#e5e5e5"];
const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
const ANSI_NAMES = [...ANSI, ...ANSI.map((c) => "bright" + c[0].toUpperCase() + c.slice(1))];
type Profile = { background: string; foreground: string; cursor: string; selectionBackground: string; ansi: string[]; font?: string; fontSize?: number };
let profile: Promise<Profile | null> | null = null; // read once per app run

/** xterm options for an appearance: the iTerm profile's colours/font, or Murim's theme tokens. */
function look(a: Appearance, p: Profile | null) {
  const css = (v: string) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();
  const useIterm = a.termColors === "iterm" && p;
  const bg = useIterm ? p.background : css("--bg") || "#0a0a0a", fg = useIterm ? p.foreground : css("--text") || "#fafafa";
  const ansi = useIterm ? p.ansi : isLight(bg) ? LIGHT : DARK;
  const theme = { ...Object.fromEntries(ansi.map((c, i) => [ANSI_NAMES[i], c])), background: bg, foreground: fg,
    cursor: useIterm ? p.cursor : css("--brand") || fg, cursorAccent: bg, selectionBackground: (useIterm ? p.selectionBackground : css("--brand") || "#888888") + "55" };
  const fontFamily = a.termFont === "iterm" ? (p?.font ? `"${p.font}", ${family("jetbrains")}` : family("jetbrains")) : family(a.termFont);
  const fontSize = a.termSize || (a.termFont === "iterm" && p?.fontSize) || 12.5;
  return { theme, fontFamily, fontSize };
}

export default function Terminal({ issueId }: { issueId: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current!;
    const term = new XTerm({ cursorBlink: true, ...look(currentAppearance(), null) });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const resize = () => { if (!term.element) return; fit.fit(); invoke("pty_resize", { issueId, cols: term.cols, rows: term.rows }).catch(() => {}); };
    // colours + font follow the iTerm profile or Murim's theme (Settings → Appearance), live
    let prof: Profile | null = null;
    const restyle = (a: Appearance) => {
      const o = look(a, prof);
      term.options.theme = o.theme;
      el.style.background = o.theme.background;
      loadFont(a.termFont).then(() => document.fonts.load(`${o.fontSize}px ${o.fontFamily}`)).catch(() => {}).finally(() => {
        if (dead) return;
        term.options.fontFamily = o.fontFamily;
        term.options.fontSize = o.fontSize;
        resize();
      });
    };
    const onLook = (e: Event) => restyle((e as CustomEvent).detail);
    window.addEventListener("murim-appearance", onLook);
    (profile ??= invoke<Profile | null>("terminal_theme").catch(() => null)).then((p) => { prof = p; if (!dead) restyle(currentAppearance()); });
    const ro = new ResizeObserver(resize);
    // xterm measures cells at open(): open only once the bundled font is loaded, or it sizes against the fallback.
    let dead = false;
    const o0 = look(currentAppearance(), null);
    document.fonts.load(`${o0.fontSize}px ${o0.fontFamily}`).catch(() => {}).finally(() => { if (!dead) { term.open(el); ro.observe(el); } });
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
    return () => { dead = true; window.removeEventListener("murim-appearance", onLook); ro.disconnect(); d.dispose(); un.then((f) => f()); term.dispose(); };
  }, [issueId]);
  return <div className="term" ref={ref} />;
}
