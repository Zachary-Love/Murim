import { marked } from "marked";
import DOMPurify from "dompurify";
import { openUrl } from "@tauri-apps/plugin-opener";

// Content is written by other people and the webview can invoke Tauri commands: always sanitize,
// and never let a link navigate the webview.
export default function Markdown({ text }: { text: string }) {
  const html = DOMPurify.sanitize(marked.parse(text, { async: false, gfm: true }));
  return (
    <div className="md" dangerouslySetInnerHTML={{ __html: html }} onClick={(e) => {
      const a = (e.target as HTMLElement).closest("a");
      if (!a) return;
      e.preventDefault();
      const h = a.getAttribute("href") ?? "";
      if (/^(https?:|mailto:)/i.test(h)) openUrl(h).catch(() => {});
    }} />
  );
}
