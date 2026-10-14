# Appearance Implementation Plan

**Goal:** Themes (System/Light/Dark/Custom), accent, texture, UI + terminal fonts, applied live (0.2.1).
**Spec:** docs/specs/2026-10-14-appearance-design.md. **Execution:** native (one connected frontend change).

1. **Backend** — `settings.rs`: `Appearance` (serde defaults = today's look) on `Settings`; `set_appearance` command;
   `save_settings` preserves it; round-trip test. `term_theme.rs`: also return the iTerm font family + size.
2. **Engine** — `src/appearance.ts`: types/defaults/presets, `lightness(hex)` (OKLab L), `tokens(a, systemDark)` →
   CSS variables (color-mix surfaces/text by contrast, accent, on-accent, status colours, texture layer, fonts),
   `applyAppearance(a)` (sets vars, caches to localStorage, dispatches `murim-appearance`), `loadFont(key)` (dynamic
   fontsource import). Check script `scripts/appearance.check.ts` (lightness, text choice, contrast bounds).
3. **CSS** — every hard-coded colour in App.css onto tokens; `--tex` background layer on rail/sidebar/main;
   root font from `--ui-font`/`--ui-size`.
4. **Boot** — inline script in index.html applies cached vars before React; App loads appearance from settings,
   follows `prefers-color-scheme` in System mode.
5. **Settings → Appearance** — preview card, theme segmented control, base + contrast (Custom), accent swatches,
   texture + intensity, UI font + size, terminal font + size, terminal colours, review badge. Save on change.
6. **Terminal** — font + colours from appearance (`iterm` profile or Murim theme), live update on `murim-appearance`.
7. **Verify** — cargo test, tsc, check script, mock screenshots (Dark vs before, Light, beige, textures, fonts).
