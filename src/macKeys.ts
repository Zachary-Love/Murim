// Kept apart from Terminal.tsx so scripts/macKeys.check.ts can run it under node.

/** iTerm's macOS bindings xterm.js lacks (table from Orca's terminal-shortcut-policy). With kitty flags on, xterm encodes
 *  Shift+Enter / Option+key itself. Returns bytes to send, "top"/"bottom" to scroll, "clear", "paste", or null for xterm's default. */
export function macKey(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">, kitty: boolean): string | null {
  const { key, metaKey: cmd, ctrlKey: ctrl, altKey: alt, shiftKey: shift } = e;
  if (cmd && !ctrl && !alt && !shift) {
    return ({ Backspace: "\x15", Delete: "\x0b", ArrowLeft: "\x01", ArrowRight: "\x05", ArrowUp: "top", ArrowDown: "bottom", k: "clear", v: "paste" } as Record<string, string>)[key] ?? null;
  }
  if (ctrl && !cmd && !alt && !shift && key === "Backspace") return "\x17";
  if (kitty || cmd || ctrl) return null;
  if (shift && !alt && key === "Enter") return "\x1b\r"; // Claude Code: newline
  if (alt && !shift && key === "Backspace") return "\x1b\x7f";
  if (alt && !shift && (key === "ArrowLeft" || key === "ArrowRight")) return key === "ArrowLeft" ? "\x1bb" : "\x1bf"; // readline word jump
  return null;
}
