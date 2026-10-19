import assert from "node:assert/strict";
import { macKey } from "../src/macKeys.ts";

const k = (key: string, m: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}, kitty = false) =>
  macKey({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...m }, kitty);

assert.equal(k("Backspace", { metaKey: true }), "\x15"); // Cmd+Backspace: kill line
assert.equal(k("ArrowLeft", { metaKey: true }), "\x01");
assert.equal(k("ArrowUp", { metaKey: true }), "top");
assert.equal(k("k", { metaKey: true }), "clear");
assert.equal(k("c", { metaKey: true }), null); // Cmd+C stays copy
assert.equal(k("Enter", { shiftKey: true }), "\x1b\r");
assert.equal(k("Enter", { shiftKey: true }, true), null); // kitty: xterm encodes it
assert.equal(k("ArrowRight", { altKey: true }), "\x1bf");
assert.equal(k("Escape"), null);
assert.equal(k("a"), null);
console.log("macKeys ok");
