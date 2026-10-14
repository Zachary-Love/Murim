// Appearance: every theme is base + accent + contrast; CSS variables are derived here and set on :root.
// Stored as an opaque blob in settings.json (`appearance`); defaults reproduce the original dark look.

export type Mode = "system" | "light" | "dark" | "custom";
export type Texture = "none" | "grain" | "paper" | "dots" | "lines";
export type Appearance = {
  mode: Mode; base: string; accent: string; contrast: number;
  texture: Texture; textureIntensity: number;
  uiFont: string; uiSize: number;
  termFont: string; termSize: number; // termFont "iterm" = the iTerm profile font; termSize 0 = its size
  termColors: "iterm" | "theme";
};

export const DEFAULTS: Appearance = {
  mode: "dark", base: "#efe6d8", accent: "#f43f5e", contrast: 50,
  texture: "none", textureIntensity: 30,
  uiFont: "jetbrains", uiSize: 13, termFont: "iterm", termSize: 0, termColors: "iterm",
};
const PRESET = { dark: "#0a0a0a", light: "#fafafa" };
export const ACCENTS = { Rose: "#f43f5e", Blue: "#3b82f6", Violet: "#8b5cf6", Green: "#22c55e", Amber: "#f59e0b", Slate: "#64748b" };

// key -> [label, CSS family, loader]; "system" and custom names need no loader
export const FONTS: Record<string, [string, string, (() => Promise<unknown>)?]> = {
  jetbrains: ["JetBrains Mono", `"JetBrains Mono Variable"`],
  geist: ["Geist", `"Geist Variable"`, () => import("@fontsource-variable/geist")],
  "geist-mono": ["Geist Mono", `"Geist Mono Variable"`, () => import("@fontsource-variable/geist-mono")],
  "plex-sans": ["IBM Plex Sans", `"IBM Plex Sans Variable"`, () => import("@fontsource-variable/ibm-plex-sans")],
  "plex-mono": ["IBM Plex Mono", `"IBM Plex Mono"`, () => import("@fontsource/ibm-plex-mono")],
  inter: ["Inter", `"Inter Variable"`, () => import("@fontsource-variable/inter")],
  system: ["System", "-apple-system, BlinkMacSystemFont"],
};
const FALLBACK = `"SF Mono", Menlo, ui-monospace, monospace`;

/** CSS font-family for a font key or a custom installed font name. */
export const family = (key: string) => `${FONTS[key]?.[1] ?? JSON.stringify(key)}, ${FALLBACK}`;
export const loadFont = (key: string) => FONTS[key]?.[2]?.().catch(() => {}) ?? Promise.resolve();

export const withDefaults = (a: unknown): Appearance => ({ ...DEFAULTS, ...(a && typeof a === "object" ? a : {}) });

/** OKLab lightness (0..1) of a #rrggbb colour. */
export function lightness(hex: string): number {
  const n = parseInt(hex.replace("#", "").slice(0, 6).padEnd(6, "0"), 16);
  const lin = (c: number) => (c /= 255) <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  const [r, g, b] = [lin((n >> 16) & 255), lin((n >> 8) & 255), lin(n & 255)];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787995 * b);
  return 0.2104542553 * l + 0.793617785 * m - 0.0040720453 * s;
}
export const isLight = (hex: string) => lightness(hex) > 0.6;

/** The base colour actually in use (presets, or System following macOS). */
export const baseOf = (a: Appearance, systemDark: boolean) =>
  a.mode === "custom" ? a.base : a.mode === "light" || (a.mode === "system" && !systemDark) ? PRESET.light : PRESET.dark;

/** contrast 0..100 -> k 0.75..1.25 (50 = the original dark look). Surfaces spread with k, text levels tighten. */
export const k = (contrast: number) => 0.75 + Math.min(100, Math.max(0, contrast)) * 0.005;
/** Text levels move less (0.9..1.1) so muted text and icons stay legible at contrast 0. */
export const kText = (contrast: number) => 0.9 + Math.min(100, Math.max(0, contrast)) * 0.002;

function texture(t: Texture, intensity: number, light: boolean): string {
  if (t === "none") return "none";
  const i = Math.min(100, Math.max(0, intensity)) / 100, ink = light ? "0" : "1";
  const svg = (freq: number, oct: number, a: number) =>
    `url("data:image/svg+xml,${encodeURIComponent(`<svg xmlns='http://www.w3.org/2000/svg' width='180' height='180'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='${freq}' numOctaves='${oct}' stitchTiles='stitch'/><feColorMatrix values='0 0 0 0 ${ink} 0 0 0 0 ${ink} 0 0 0 0 ${ink} 0 0 0 ${a.toFixed(3)} 0'/></filter><rect width='100%' height='100%' filter='url(#n)'/></svg>`)}")`;
  const c = (a: number) => light ? `rgba(0,0,0,${a.toFixed(3)})` : `rgba(255,255,255,${a.toFixed(3)})`;
  if (t === "grain") return svg(0.85, 2, 0.05 + 0.3 * i);
  if (t === "paper") return svg(0.035, 4, 0.04 + 0.22 * i);
  if (t === "dots") return `radial-gradient(${c(0.03 + 0.12 * i)} 1px, transparent 1.4px) 0 0 / 14px 14px`;
  return `repeating-linear-gradient(0deg, ${c(0.02 + 0.08 * i)} 0 1px, transparent 1px 7px)`;
}

/** All CSS variables for an appearance. */
export function tokens(a: Appearance, systemDark: boolean): Record<string, string> {
  const base = baseOf(a, systemDark), light = isLight(base), K = k(a.contrast), T = kText(a.contrast);
  const fg = light ? "#141414" : "#fafafa";
  const mix = (pct: number) => `color-mix(in srgb, ${fg} ${Math.min(100, pct).toFixed(1)}%, ${base})`;
  const status = light
    ? { red: "#dc2626", amber: "#b45309", blue: "#2563eb", green: "#16a34a" }
    : { red: "#ef6b6b", amber: "#f0b429", blue: "#5aa9e6", green: "#4ade80" };
  return {
    "--base": base, "--fg": fg, "--bg": base,
    "--surface": mix(3.3 * K), "--raised": mix(5.4 * K), "--hover": mix(8.7 * K), "--line": mix(11.7 * K), "--line-hi": mix(19 * K),
    "--text": fg, "--text-2": mix(100 - 16 / T), "--muted": mix(100 - 36 / T), "--dim": mix(100 - 56 / T),
    "--accent": a.accent, "--brand": a.accent, "--brand-dim": `color-mix(in srgb, ${a.accent} 12%, transparent)`,
    "--on-accent": isLight(a.accent) ? "#141414" : "#ffffff",
    "--red": status.red, "--amber": status.amber, "--blue": status.blue, "--green": status.green,
    "--tex": texture(a.texture, a.textureIntensity, light),
    "--ui-font": family(a.uiFont), "--ui-size": `${a.uiSize}px`,
    "color-scheme": light ? "light" : "dark",
  };
}

const CACHE = "murim.appearance.vars";
export const systemDark = () => matchMedia("(prefers-color-scheme: dark)").matches;

let current = DEFAULTS;
/** The appearance last applied (terminals read it on mount, then follow `murim-appearance`). */
export const currentAppearance = () => current;

/** Set the variables on :root, cache them for a flash-free next launch (index.html), tell terminals. */
export function applyAppearance(a: Appearance) {
  current = a;
  const t = tokens(a, systemDark()), root = document.documentElement.style;
  for (const [k, v] of Object.entries(t)) root.setProperty(k, v);
  try { localStorage.setItem(CACHE, JSON.stringify(t)); } catch {}
  loadFont(a.uiFont);
  window.dispatchEvent(new CustomEvent("murim-appearance", { detail: a }));
}
