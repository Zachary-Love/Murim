import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ReviewPr, ReviewFilters, Bucket, BUCKET_LABEL, SEV, OVERALL } from "./types";
import { applyFilters, authorsOf, authorKey, authorLabel } from "./reviewFilter";
import { PrIcon } from "./Icons";

// 16px round avatar: Linear picture, else initials (dashed ring for unlinked GitHub authors)
export const Avatar = ({ url, label, unlinked }: { url?: string | null; label: string; unlinked?: boolean }) =>
  url ? <img className="avatar" src={url} alt="" /> : <span className={`avatar ${unlinked ? "unlinked" : ""}`}>{label.replace("@", "").slice(0, 1).toUpperCase()}</span>;

const BUCKETS = Object.keys(BUCKET_LABEL) as Bucket[];
type Dim = "authors" | "statuses";

export default function ReviewList({ prs, error, open, onOpen }: { prs: ReviewPr[]; error: string | null; open: string | null; onOpen: (key: string) => void }) {
  const [f, setF] = useState<ReviewFilters>({ authors: [], statuses: [] });
  const [pop, setPop] = useState<Dim | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    invoke<{ review_filters?: ReviewFilters }>("get_settings").then((s) => s.review_filters && setF({ authors: s.review_filters.authors ?? [], statuses: s.review_filters.statuses ?? [] })).catch(() => {});
  }, []);
  useEffect(() => {
    if (!pop) return;
    const close = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setPop(null); };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [pop]);
  const save = (v: ReviewFilters) => { setF(v); invoke("set_review_filters", { filters: v }).catch(() => {}); };
  const toggle = (d: Dim, k: string) => save({ ...f, [d]: f[d].includes(k as never) ? f[d].filter((x) => x !== k) : [...f[d], k] });
  const authors = authorsOf(prs);
  const shown = applyFilters(prs, f);
  // a remembered author may not be in the current list: label it by key rather than hide it
  const aName = (k: string) => authors.find((a) => a.key === k)?.label ?? (k === "unlinked" ? "Unlinked" : "Unknown");
  const active = f.authors.length + f.statuses.length > 0;
  return (
    <>
      <div className="filter-bar" ref={ref}>
        <span className="anchor">
          <button className={`btn ${pop === "authors" ? "on" : ""}`} onClick={() => setPop(pop === "authors" ? null : "authors")}>Author{f.authors.length > 0 && <span className="count">{f.authors.length}</span>}</button>
          {pop === "authors" && (
            <div className="popover">
              {!authors.length && <span className="empty">No authors</span>}
              {authors.map((a) => (
                <button key={a.key} className={f.authors.includes(a.key) ? "on" : ""} onClick={() => toggle("authors", a.key)}>
                  <span className="check">{f.authors.includes(a.key) && "✓"}</span><Avatar url={a.avatarUrl} label={a.label} unlinked={a.key === "unlinked"} />{a.label}
                </button>
              ))}
            </div>
          )}
        </span>
        <span className="anchor">
          <button className={`btn ${pop === "statuses" ? "on" : ""}`} onClick={() => setPop(pop === "statuses" ? null : "statuses")}>Status{f.statuses.length > 0 && <span className="count">{f.statuses.length}</span>}</button>
          {pop === "statuses" && (
            <div className="popover">
              {BUCKETS.map((b) => (
                <button key={b} className={f.statuses.includes(b) ? "on" : ""} onClick={() => toggle("statuses", b)}>
                  <span className="check">{f.statuses.includes(b) && "✓"}</span>{BUCKET_LABEL[b]}
                </button>
              ))}
            </div>
          )}
        </span>
        {active && <button className="link-btn clear" onClick={() => save({ authors: [], statuses: [] })}>Clear</button>}
      </div>
      {active && (
        <div className="filter-chips">
          {f.authors.length > 0 && <span className="chip">Author: {f.authors.map(aName).join(", ")}<button title="Clear author filter" onClick={() => save({ ...f, authors: [] })}>×</button></span>}
          {f.statuses.length > 0 && <span className="chip">Status: {f.statuses.map((b) => BUCKET_LABEL[b]).join(", ")}<button title="Clear status filter" onClick={() => save({ ...f, statuses: [] })}>×</button></span>}
        </div>
      )}
      {error && <div className="banner">{error}</div>}
      <ul className="tasks">
        {shown.map((p) => (
          <li key={p.key}>
            <button className={`row ${open === p.key ? "on" : ""}`} title={`${p.health.title} · ${authorLabel(p.author)}`} onClick={() => onOpen(p.key)}>
              <Avatar url={p.author.kind === "linear" ? p.author.avatarUrl : null} label={authorLabel(p.author)} unlinked={authorKey(p.author) === "unlinked"} />
              <span className="ident">{p.repo}#{p.number}</span>
              <span className="t">{p.health.title}</span>
              {p.requested && <i className="ico dot-req" title="Your review is requested" />}
              <PrIcon color={SEV[p.health.overall] ?? "grey"} title={OVERALL[p.health.overall] ?? p.health.overall} />
            </button>
          </li>
        ))}
        {!prs.length && <li className="empty">Nothing to review</li>}
        {prs.length > 0 && !shown.length && <li className="empty">No PRs match these filters</li>}
      </ul>
    </>
  );
}
