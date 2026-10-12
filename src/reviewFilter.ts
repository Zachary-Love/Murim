import type { ReviewPr, ReviewAuthor, ReviewFilters } from "./types";

export const authorKey = (a: ReviewAuthor) => (a.kind === "linear" ? a.id : "unlinked");
export const authorLabel = (a: ReviewAuthor) => (a.kind === "linear" ? a.name : `@${a.login}`);

// Linear semantics: empty dimension = no filter; OR within a dimension, AND across.
export const applyFilters = (prs: ReviewPr[], f: ReviewFilters) =>
  prs.filter((p) => (!f.authors.length || f.authors.includes(authorKey(p.author))) && (!f.statuses.length || f.statuses.includes(p.bucket)));

export const authorsOf = (prs: ReviewPr[]) => {
  const m = new Map<string, { key: string; label: string; avatarUrl: string | null }>();
  for (const p of prs) {
    const k = authorKey(p.author);
    if (!m.has(k)) m.set(k, { key: k, label: k === "unlinked" ? "Unlinked" : authorLabel(p.author), avatarUrl: p.author.kind === "linear" ? p.author.avatarUrl : null });
  }
  return [...m.values()].sort((a, b) => (a.key === "unlinked" ? 1 : b.key === "unlinked" ? -1 : a.label.localeCompare(b.label)));
};
