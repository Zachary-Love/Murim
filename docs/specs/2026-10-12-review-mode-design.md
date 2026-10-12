# Review mode — design

Status: approved in chat 2026-10-12.

## Goal

A second sidebar mode for reviewing other people's PRs, filterable like Linear (author as a **Linear user**, status),
with an agent that drafts the review and nothing posted until the user clicks.

## Layout

- New **icon rail** (~44px, icons only) left of the existing sidebar: **Tasks** (current list) and **Reviews**.
  Settings and Caffeine move to the rail's bottom. macOS traffic lights sit above the rail; the rail top is a drag region.
- Tasks mode: unchanged sidebar + TaskPane.
- Reviews mode: sidebar shows a filter bar + PR list; the main pane shows the selected PR (ReviewPane).
- Selected mode and selected PR are remembered per mode (switching back restores the selection).

## Data: which PRs

Union, de-duplicated by PR URL:

1. **GitHub requested:** GraphQL search `is:pr is:open review-requested:@me` (covers direct and team requests the
   viewer belongs to). These carry `requested: true`.
2. **Linear team:** issues in the viewer's teams, state type `started`, with GitHub PR attachments
   (attachment URL matches `github.com/<owner>/<repo>/pull/<n>`).

Excluded: PRs authored by the viewer (GitHub `author.login == viewer.login`), closed/merged PRs.

PR status comes from the existing `github::parse_pr` / `PrHealth` (CI, review decision, draft, conflict, threads).
Polling piggybacks on the existing GitHub poller cadence; a manual refresh button exists in the list.

## Author → Linear user

Per PR, first hit wins:

1. Linear issue attached to the PR → issue **assignee** (id, name, avatarUrl).
2. Branch name contains a Linear identifier (`[a-z]+-\d+`, case-insensitive, e.g. `eng-123-fix`) → look up that issue →
   assignee.
3. Otherwise **unlinked**: author shown as the GitHub login, grouped under "Unlinked" in the filter.

Linear lookups are cached per app run (identifier → assignee).

## Filters

- **Author:** multi-select of Linear users seen in the current list (avatar + name) plus "Unlinked".
- **Status:** Needs review, Draft, CI failing, Changes requested, Approved. Mapping from `Overall`:
  Needs review = `awaitingReview` | `ciPending` | `unresolvedComments`; Draft = `draft`; CI failing = `ciFailing` |
  `conflict`; Changes requested = `changesRequested`; Approved = `readyToMerge`.
- Empty selection in a dimension = no filter. Dimensions AND together; values within a dimension OR.
- Last-used filters persist in Settings (`review_filters: { authors: [ids], statuses: [..] }`, unlinked = `"unlinked"`).

## Review pane

Tabs:

- **Overview:** title, author (Linear user), Requested marker, description (Markdown), CI/status chips, linked Linear
  issue link.
- **Diff:** existing diff viewer component, fed by `gh pr diff <url>` (same 1.5MB cap as review.rs).
- **Agent:** terminal + draft review.

**Review with agent:**

1. Find the local clone: repo dir under `repos_root` whose `origin` matches `owner/repo` (reuse `parse_remote`).
   Missing → error "Clone <owner/repo> under your repos folder first".
2. `git fetch origin pull/<n>/head:murim-review-<n>` then `git worktree add ~/.murim/reviews/<repo>/<n> murim-review-<n>`
   (reuse if present; refetch + reset on re-run).
3. Spawn the configured agent there (same PTY/session machinery, session id `review:<owner>/<repo>#<n>`) with a review
   prompt: read the diff against the base, write the review to `.murim/review.json`
   (`{ "summary": string, "event": "COMMENT"|"APPROVE"|"REQUEST_CHANGES", "comments": [{ "path", "line", "body" }] }`),
   do **not** post, push or comment on GitHub. `.murim/` is git-excluded as for task worktrees.
4. When the file appears (or the agent stops), the pane shows an editable draft: summary textarea, event select,
   comment list (editable/removable).
5. **Post** buttons (Approve / Request changes / Comment) submit via the GitHub REST reviews endpoint
   (`gh api repos/<o>/<r>/pulls/<n>/reviews` with body, event, comments) only on click.
6. Review worktree + `murim-review-<n>` branch are removed when the PR closes/merges or on **Dismiss**.

Custom agents get the same prompt as their last argument; they must also write `.murim/review.json`.

## Error handling

- gh logged out / Linear key missing → banner in the review list, same style as the Tasks list.
- `review.json` invalid → show raw file content and a message; posting disabled until fixed.
- Comments on lines outside the diff are rejected by GitHub → show the API error, keep the draft.

## Testing

- Unit: author resolution order (attachment > branch id > unlinked), identifier regex, Overall→status-filter mapping,
  filter AND/OR semantics, review.json parse, review worktree creation on a temp repo with a fake `pull/1/head` ref.
- Manual: real GitHub search, Linear attachments, agent draft, posting.

## Not in scope

Named saved views; team/project/cycle/repo/label filters; assigning reviewers; inline comment threads UI beyond the draft list.
