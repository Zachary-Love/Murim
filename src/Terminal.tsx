import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import { Appearance, currentAppearance, family, loadFont } from "./appearance";
import { macKey } from "./macKeys";

// The terminal should feel like the user's own iTerm, not part of Murim's theme (setup follows Orca's, also xterm.js):
// iTerm profile colours, GPU rendering, Unicode 11 widths, kitty keyboard protocol, and iTerm's macOS key bindings.

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
// iTerm2's default dark palette: the fallback without an iTerm profile
const ITERM_DARK = ["#000000", "#c91b00", "#00c200", "#c7c400", "#0225c7", "#c930c7", "#00c5c7", "#c7c7c7", "#686868", "#ff6e67", "#5ffa68", "#fffc67", "#6871ff", "#ff77ff", "#60fdff", "#ffffff"];
const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
const ANSI_NAMES = [...ANSI, ...ANSI.map((c) => "bright" + c[0].toUpperCase() + c.slice(1))];
type Profile = { background: string; foreground: string; cursor: string; selectionBackground: string; selectionForeground?: string; ansi: string[]; font?: string; fontSize?: number };
const FALLBACK: Profile = { background: "#000000", foreground: "#c7c7c7", cursor: "#c7c7c7", selectionBackground: "#c1deff", selectionForeground: "#000000", ansi: ITERM_DARK };
let profile: Promise<Profile | null> | null = null; // read once per app run

/** xterm options: the iTerm profile's colours; font from Settings → Appearance (default: the profile's). */
function look(a: Appearance, p: Profile | null) {
  const c = p ?? FALLBACK;
  const theme = { ...Object.fromEntries(c.ansi.map((x, i) => [ANSI_NAMES[i], x])), background: c.background, foreground: c.foreground,
    cursor: c.cursor, cursorAccent: c.background, selectionBackground: c.selectionBackground, selectionForeground: c.selectionForeground };
  const fontFamily = a.termFont === "iterm" ? (p?.font ? `"${p.font}", ${family("jetbrains")}` : family("jetbrains")) : family(a.termFont);
  const fontSize = a.termSize || (a.termFont === "iterm" && p?.fontSize) || 12.5;
  return { theme, fontFamily, fontSize };
}

export default function Terminal({ issueId }: { issueId: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current!;
    // ⌘-click, like Terminal.app and iTerm; a plain click stays a click (selection, the agent's TUI)
    const openLink = (e: MouseEvent, uri: string) => { if (e.metaKey && /^https?:\/\//.test(uri)) openUrl(uri).catch(() => {}); };
    let dead = false;
    let prof: Profile | null = null;
    const term = new XTerm({ cursorBlink: true, allowProposedApi: true, macOptionIsMeta: false, macOptionClickForcesSelection: true, vtExtensions: { kittyKeyboard: true }, linkHandler: { activate: (e, uri) => openLink(e, uri) }, ...look(currentAppearance(), null) });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new Unicode11Addon());
    term.loadAddon(new WebLinksAddon(openLink)); // plain-text URLs; OSC 8 links go through linkHandler above
    term.unicode.activeVersion = "11";
    let webgl: WebglAddon | null = null;
    const send = (data: string) => invoke("pty_write", { issueId, data }).catch(() => {});
    // ponytail: xterm keeps the kitty flags internally; no public getter in 6.1
    const kitty = () => ((term as unknown as { _core?: { coreService?: { kittyKeyboard?: { flags?: number } } } })._core?.coreService?.kittyKeyboard?.flags ?? 0) > 0;
    term.attachCustomKeyEventHandler((e) => {
      if (e.isComposing) return true;
      const k = macKey(e, kitty());
      if (k === null) return true;
      if (e.type === "keydown") {
        e.preventDefault();
        if (k === "top") term.scrollToTop(); else if (k === "bottom") term.scrollToBottom(); else if (k === "clear") term.clear();
        // Cmd+V ourselves: the menu's paste only lands when WebKit has the xterm textarea focused, which some panes lose
        else if (k === "paste") invoke<string>("clipboard_text").then((t) => term.paste(t)).catch(() => {});
        else send(k);
      }
      return false;
    });
    const resize = () => { if (!term.element) return; fit.fit(); invoke("pty_resize", { issueId, cols: term.cols, rows: term.rows }).catch(() => {}); };
    // font follows Settings → Appearance live; colours are always the iTerm profile
    const restyle = (a: Appearance) => {
      const o = look(a, prof);
      term.options.theme = o.theme;
      el.style.background = o.theme.background;
      loadFont(a.termFont).then(() => document.fonts.load(`${o.fontSize}px ${o.fontFamily}`)).catch(() => {}).finally(() => {
        if (dead) return;
        term.options.fontFamily = o.fontFamily;
        term.options.fontSize = o.fontSize;
        webgl?.clearTextureAtlas();
        resize();
      });
    };
    const onLook = (e: Event) => restyle((e as CustomEvent).detail);
    window.addEventListener("murim-appearance", onLook);
    const ro = new ResizeObserver(resize);
    // open only once the profile and font are in: xterm measures cells at open(), and no flash of fallback colours
    (profile ??= invoke<Profile | null>("terminal_theme").catch(() => null)).then(async (p) => {
      prof = p;
      const o = look(currentAppearance(), p);
      term.options.theme = o.theme;
      term.options.fontFamily = o.fontFamily;
      term.options.fontSize = o.fontSize;
      el.style.background = o.theme.background;
      await loadFont(currentAppearance().termFont);
      await document.fonts.load(`${o.fontSize}px ${o.fontFamily}`).catch(() => {});
      if (dead) return;
      term.open(el);
      try {
        webgl = new WebglAddon();
        webgl.onContextLoss(() => { webgl?.dispose(); webgl = null; }); // falls back to the DOM renderer
        term.loadAddon(webgl);
      } catch { webgl = null; }
      ro.observe(el);
      term.focus();
      // Replay only now, at the real size: TUIs like Claude Code draw with width-relative cursor moves, so history
      // written at xterm's default 80x24 and then reflowed comes out scrambled.
      resize();
      const snap = await invoke<{ data: string; end: number }>("pty_snapshot", { issueId }).catch(() => ({ data: "", end: 0 }));
      if (dead) return;
      await new Promise<void>((done) => term.write(bytes(snap.data), done));
      replaying = false;
      // live chunks the snapshot already holds would draw a frame twice ("move up N lines" twice shifts the screen)
      pending.filter((c) => c.end > snap.end).forEach((c) => term.write(c.b));
      ready = true;
    });
    // Buffer live output until the snapshot is written so ordering holds.
    let ready = false;
    const pending: { b: Uint8Array; end: number }[] = [];
    const un = listen<{ issueId: string; data: string; end: number }>("pty-output", (e) => {
      if (e.payload.issueId !== issueId) return;
      const b = bytes(e.payload.data);
      ready ? term.write(b) : pending.push({ b, end: e.payload.end });
    });
    // Replaying history re-runs old terminal queries (device attributes, kitty keyboard); their answers are stale, drop them.
    let replaying = true;
    const d = term.onData((data) => { if (!replaying) send(data); });
    return () => { dead = true; window.removeEventListener("murim-appearance", onLook); ro.disconnect(); d.dispose(); un.then((f) => f()); term.dispose(); };
  }, [issueId]);
  return <div className="term" ref={ref} />;
}
