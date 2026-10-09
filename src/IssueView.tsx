import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Markdown from "./Markdown";

type Detail = { description: string; stateName: string; priorityLabel: string; labels: string[]; assignee: string; comments: { author: string; body: string; createdAt: string }[] };

export default function IssueView({ issueId }: { issueId: string }) {
  const [d, setD] = useState<Detail | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    invoke<Detail>("get_issue_detail", { issueId }).then(setD).catch((e) => setErr(String(e)));
  }, [issueId]);
  if (err) return <div className="banner">{err}</div>;
  if (!d) return <p className="empty">Loading…</p>;
  const meta = [d.stateName, d.priorityLabel, d.labels.join(", "), d.assignee].filter(Boolean).join(" · ");
  return (
    <div className="issue">
      <div className="meta">{meta}</div>
      {d.description ? <Markdown text={d.description} /> : <p className="empty">No description</p>}
      {d.comments.map((c, n) => (
        <div key={n} className="comment">
          <div className="meta">{c.author || "Unknown"} · {new Date(c.createdAt).toLocaleString()}</div>
          <Markdown text={c.body} />
        </div>
      ))}
    </div>
  );
}
