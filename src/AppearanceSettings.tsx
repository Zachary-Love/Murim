import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Appearance, ACCENTS, FONTS, Texture, applyAppearance, withDefaults } from "./appearance";

const MODES: [Appearance["mode"], string][] = [["system", "System"], ["light", "Light"], ["dark", "Dark"], ["custom", "Custom"]];
const TEXTURES: Texture[] = ["none", "grain", "paper", "dots", "lines"];
const cap = (s: string) => s[0].toUpperCase() + s.slice(1);

// Font picker: bundled keys + "System" + any installed font typed by name. `extra` adds leading options (terminal: iTerm).
function FontPick({ value, onChange, extra = [] }: { value: string; onChange: (v: string) => void; extra?: [string, string][] }) {
  const known = value in FONTS || extra.some(([k]) => k === value);
  const [custom, setCustom] = useState(!known);
  return (
    <div className="group gap">
      <select value={custom ? "custom" : value} onChange={(e) => {
        const v = e.target.value;
        setCustom(v === "custom");
        if (v !== "custom") onChange(v);
      }}>
        {extra.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
        {Object.entries(FONTS).map(([k, [l]]) => <option key={k} value={k}>{l}</option>)}
        <option value="custom">Installed font…</option>
      </select>
      {custom && <input className="grow" defaultValue={known ? "" : value} placeholder="Font name, e.g. Menlo" onBlur={(e) => e.target.value.trim() && onChange(e.target.value.trim())}
        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); (e.target as HTMLInputElement).blur(); } }} />}
    </div>
  );
}

// Every change applies live and is saved (debounced: colour pickers and sliders fire continuously).
export default function AppearanceSettings() {
  const [a, setA] = useState<Appearance | null>(null);
  const [iterm, setIterm] = useState<{ font?: string; fontSize?: number } | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => {
    invoke<{ appearance: unknown }>("get_settings").then((s) => setA(withDefaults(s.appearance))).catch(() => setA(withDefaults(null)));
    invoke<{ font?: string; fontSize?: number } | null>("terminal_theme").then(setIterm).catch(() => {});
  }, []);
  if (!a) return null;
  const set = (p: Partial<Appearance>) => {
    const next = { ...a, ...p };
    setA(next);
    applyAppearance(next);
    clearTimeout(timer.current);
    timer.current = window.setTimeout(() => invoke("set_appearance", { appearance: next }).catch(() => {}), 250);
  };
  const itermLabel = iterm?.font ? `iTerm (${iterm.font}${iterm.fontSize ? ` ${iterm.fontSize}` : ""})` : "iTerm profile";
  return (
    <>
      <div className="preview">
        <div className="preview-side">
          <div className="section-label">My tasks</div>
          <div className="row on"><span className="ident">MUR-7</span><span className="t">Write the docs</span></div>
          <div className="row"><span className="ident">MUR-3</span><span className="t">Fix flaky CI</span></div>
        </div>
        <div className="preview-main">
          <div className="group gap"><button type="button" className="btn primary">Start agent</button><button type="button" className="btn">Resume</button><span className="chip green">CI passing</span></div>
          <code>git commit -m "fix: retry webhooks"</code>
          <span className="muted">Muted text · <span style={{ color: "var(--brand)" }}>accent</span></span>
        </div>
      </div>

      <div className="lab">Theme
        <div className="seg flat">{MODES.map(([m, l]) => <button type="button" key={m} className={a.mode === m ? "on" : ""} onClick={() => set({ mode: m })}>{l}</button>)}</div>
      </div>
      {a.mode === "custom" && <>
        <div className="lab">Base colour
          <div className="group gap">
            <input type="color" value={a.base} onChange={(e) => set({ base: e.target.value })} />
            <input className="mono hex" value={a.base} onChange={(e) => /^#[0-9a-f]{6}$/i.test(e.target.value) && set({ base: e.target.value })} />
          </div>
        </div>
        <div className="lab"><span>Contrast · {a.contrast}</span>
          <input type="range" min={0} max={100} value={a.contrast} onChange={(e) => set({ contrast: +e.target.value })} />
        </div>
      </>}
      <div className="lab">Accent
        <div className="group gap swatches">
          {Object.entries(ACCENTS).map(([n, c]) => <button type="button" key={n} title={n} className={`swatch ${a.accent.toLowerCase() === c ? "on" : ""}`} style={{ background: c }} onClick={() => set({ accent: c })} />)}
          <input type="color" title="Any colour" value={a.accent} onChange={(e) => set({ accent: e.target.value })} />
        </div>
      </div>
      <div className="lab">Texture
        <div className="group gap">
          <select value={a.texture} onChange={(e) => set({ texture: e.target.value as Texture })}>{TEXTURES.map((t) => <option key={t} value={t}>{cap(t)}</option>)}</select>
          {a.texture !== "none" && <input className="grow" type="range" min={0} max={100} value={a.textureIntensity} title={`Intensity ${a.textureIntensity}`} onChange={(e) => set({ textureIntensity: +e.target.value })} />}
        </div>
      </div>
      <div className="lab">Interface font
        <div className="group gap">
          <FontPick value={a.uiFont} onChange={(uiFont) => set({ uiFont })} />
          <input className="size" type="number" min={11} max={16} value={a.uiSize} onChange={(e) => set({ uiSize: Math.min(16, Math.max(11, +e.target.value || 13)) })} />
        </div>
      </div>
      <div className="lab">Terminal font
        <div className="group gap">
          <FontPick value={a.termFont} extra={[["iterm", itermLabel]]} onChange={(termFont) => set({ termFont })} />
          <input className="size" type="number" min={0} max={20} value={a.termSize || ""} placeholder={String(iterm?.fontSize ?? 12.5)} title="Empty = the iTerm size"
            onChange={(e) => set({ termSize: Math.min(20, Math.max(0, +e.target.value || 0)) })} />
        </div>
      </div>
    </>
  );
}
