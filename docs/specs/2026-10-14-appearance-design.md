# Appearance — design (0.2.1)

Status: approved in chat 2026-10-14.

## Goal

Customizable look: System / Light / Dark / Custom (Linear-style base + accent + contrast, e.g. a beige UI), accent
colour, optional texture, and separate UI and terminal fonts. Everything applies live.

## Theme model

Every theme is three inputs: `base` (background colour), `accent`, `contrast` (0–100). CSS derives all tokens:

```
--base, --fg      (fg = near-black or near-white, chosen in JS from base lightness: OKLCH L > 0.6 → dark text)
--bg      = base
--surface = color-mix(in oklch, base, fg  calc(3% * k))       k = contrast scale (contrast 50 → 1)
--raised  = mix 5%·k   --hover = mix 8%·k   --line = mix 12%·k   --line-hi = mix 20%·k
--text    = mix(fg, base 2%)   --text-2 = 85% fg   --muted = 64% fg   --dim = 46% fg   (fg-in-base percentages scale with contrast)
--brand / --accent = accent   --brand-dim = accent at 12% alpha
--red --amber --blue --green: fixed per light/dark (status colours stay legible)
color-scheme: dark | light (from base lightness)
```

Presets: **Dark** = base `#0a0a0a`, accent `#f43f5e`, contrast 50 (≈ today's look). **Light** = base `#fafafa`,
accent `#e11d48`, contrast 50. **System** = Light/Dark following macOS (`prefers-color-scheme`, live).
**Custom** = user base + accent + contrast.

All ~18 hard-coded colours in App.css move onto tokens (or onto fixed status colours) so every theme is complete.
Exact mix percentages are tuned against screenshots so Dark matches the current UI closely.

## Accent

Swatches: rose `#f43f5e`, blue `#3b82f6`, violet `#8b5cf6`, green `#22c55e`, amber `#f59e0b`, slate `#64748b`, plus a
colour input. Used for brand, primary buttons, selection, focus ring, rail active icon, badges. Applies in every theme
mode (Light/Dark presets' accent is just the default).

## Texture

`none | grain | paper | dots | lines`, intensity 0–100 (default 30 when first picked). Pure CSS: `.app::before`
overlay, `pointer-events: none`, `mix-blend-mode` soft-light/multiply per light/dark, opacity from intensity.
Grain/paper = inline SVG `feTurbulence` data URIs (different base frequency); dots/lines = gradients. Painted over
backgrounds only: the terminal, inputs and the markdown/diff code blocks sit above it (opaque, higher stacking).

## Fonts

- **UI font**: JetBrains Mono (default), Geist, Geist Mono, IBM Plex Sans, IBM Plex Mono, Inter, System
  (`-apple-system`), or Custom (any installed font by name). UI size 12–15 (default 13).
- **Terminal font**: default = the iTerm profile font (from `terminal_theme`, e.g. Monaco 12), else JetBrains Mono;
  same list + Custom; size 10–18.
- Bundled via `@fontsource-variable/*` (geist, geist-mono, ibm-plex-sans, ibm-plex-mono, inter) — loaded with dynamic
  `import()` only when selected. The terminal waits for its font (existing `document.fonts.load`) and refits on change.

## Terminal colours

`terminalColors: "iterm" | "theme"`. `iterm` = current behaviour (falls back to theme when no iTerm profile).
`theme` = xterm background/foreground/cursor/selection from Murim tokens; ANSI palette = iTerm default dark or a light
variant by base lightness. Live update of open terminals on change.

## Settings UI (Appearance section)

Preview card (sidebar row, primary button, chip, a mono code line) on top; then Theme (segmented: System, Light, Dark,
Custom), Custom → base colour, contrast slider; Accent swatches; Texture select + intensity slider; UI font + size;
Terminal font + size; Terminal colours select; the existing Review requests badge. All controls save on change.

## Storage & application

`settings.json` gains `appearance: { mode, base, accent, contrast, texture, texture_intensity, ui_font, ui_size,
term_font, term_size, term_colors }` (serde defaults = today's look), saved via `set_appearance(appearance)` without
touching other settings. Frontend `applyAppearance(a)` sets CSS variables on `:root` and dispatches
`murim-appearance` (window event) so terminals update. To avoid a flash at launch, the last applied tokens are cached
in `localStorage` and applied by an inline script in `index.html` before React mounts.

## Testing

- Unit (node check script): base lightness → text colour (beige → dark text, navy → light text, `#808080` edge);
  contrast scale bounds; settings round-trip defaults.
- Rust: appearance serde defaults + round trip.
- Mock screenshots: Dark (compare with current), Light, Custom beige, each texture, Geist/IBM Plex UI font, terminal
  colours `theme` on beige.

## Not in scope

Named saved themes; import/export; per-section colours; texture inside the terminal.
