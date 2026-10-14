// node scripts/appearance.check.ts — lightness / text choice / contrast bounds / defaults
import assert from "node:assert/strict";
import { isLight, k, kText, tokens, withDefaults, baseOf, DEFAULTS } from "../src/appearance.ts";

assert.equal(isLight("#efe6d8"), true, "beige is light -> dark text");
assert.equal(isLight("#1e293b"), false, "navy is dark -> light text");
assert.equal(isLight("#fafafa"), true);
assert.equal(isLight("#0a0a0a"), false);
assert.equal(isLight("#808080"), false, "mid grey (L~0.6) counts as dark");
assert.equal(k(50), 1);
assert.equal(k(-10), 0.75);
assert.equal(k(500), 1.25);
assert.ok(100 - 56 / kText(0) > 35, "dim text stays legible at contrast 0");
const beige = tokens({ ...DEFAULTS, mode: "custom", base: "#efe6d8" }, true);
assert.equal(beige["--fg"], "#141414");
assert.equal(beige["color-scheme"], "light");
assert.equal(tokens(DEFAULTS, false)["--bg"], "#0a0a0a", "default stays the original dark");
assert.equal(baseOf({ ...DEFAULTS, mode: "system" }, false), "#fafafa");
assert.equal(withDefaults(null).uiFont, "jetbrains");
assert.equal(withDefaults({ accent: "#3b82f6" }).accent, "#3b82f6");
assert.equal(tokens({ ...DEFAULTS, accent: "#f59e0b" }, true)["--on-accent"], "#141414", "amber accent gets dark button text");
console.log("appearance ok");
