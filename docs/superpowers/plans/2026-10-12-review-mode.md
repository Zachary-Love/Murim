# Review Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An icon rail that switches the sidebar between Tasks and a Reviews list of other people's PRs (filterable by Linear user and status), with an agent that drafts a review that is posted only when the user clicks.

**Architecture:** New Rust module `src-tauri/src/reviews.rs` polls GitHub (review-requested search) and Linear (team issues with PR attachments), resolves each PR's author to a Linear user, and emits `reviews-updated`. Review worktrees are detached checkouts of `refs/murim/pr-<n>` under `~/.murim/reviews/`, run through the existing PTY session machinery with session id `review:<owner>/<repo>/<n>`. Frontend adds a rail in `App.tsx`, `ReviewList.tsx` (filters), and `ReviewPane.tsx` (Overview / Diff / Agent with an editable draft).

**Tech Stack:** Tauri v2, Rust (ureq, serde_json, portable-pty), React 19 + TS, xterm.js, `gh` CLI, Linear GraphQL, GitHub GraphQL.

**Spec:** `docs/specs/2026-10-12-review-mode-design.md`

## Global Constraints

- No new dependencies (Rust or npm).
- Commits: never add `Co-Authored-By: Claude` or "Generated with Claude Code". Do not push.
- Settings JSON is snake_case on the wire; event/command payload structs are `#[serde(rename_all = "camelCase")]`.
- Style: shortest correct diff, reuse existing helpers (`github::gql`, `github::token`, `github::parse_remote`, `tempering::gh`, `sessions::git`, `sessions::spawn`, `sessions::write_hooks`, `sessions::exclude_settings`, `repo_match::list_repos`), match surrounding density; mark deliberate shortcuts with a `ponytail:` comment.
- Verification commands: `cd src-tauri && cargo test` and, from repo root, `pnpm exec tsc --noEmit`.
- Status filter mapping (verbatim from spec): Needs review = `awaitingReview` | `ciPending` | `unresolvedComments`; Draft = `draft`; CI failing = `ciFailing` | `conflict`; Changes requested = `changesRequested`; Approved = `readyToMerge`.
- Filters: empty selection in a dimension = no filter; dimensions AND; values within a dimension OR. Unlinked author key = `"unlinked"`.
- `review.json` shape: `{ "summary": string, "event": "COMMENT"|"APPROVE"|"REQUEST_CHANGES", "comments": [{ "path", "line", "body" }] }`.
- Diff cap: same 1.5MB (`MAX_DIFF = 1_500_000`) as `review.rs`.

## Review Focus

1. Linear key missing or Linear down while GitHub works → GitHub-requested PRs still listed (author falls back to unlinked), banner shows the Linear error. Test: `combine_partial_failure` in Task 3.
2. The same PR arrives from both sources → one row, `requested: true`, and the Linear author/issue kept. Test: `merge_dedupes_and_keeps_linear_author` in Task 2.
3. Agent leaves `review.json` half-written or invalid → pane shows the error and raw text, posting disabled, no crash. Test: `parse_draft_rejects_bad` in Task 2.
4. A poll fails (offline) → review worktrees/sessions are NOT cleaned up as "closed". Test: `stale_only_after_success` in Task 4.
5. PR repo not cloned under the repos folder → "Clone owner/repo under your repos folder first", nothing created. Test: `find_clone_matches_origin` (negative case) in Task 4.

---

### Task 1: Share the PR GraphQL selection and per-node parser

**Files:**
- Modify: `src-tauri/src/github.rs` (QUERY const ~line 181, `parse_pr` ~line 108, `query` fn, `poll` loop ~line 233)

**Interfaces:**
- Produces: `pub const PR_FIELDS: &str`; `pub fn pr_node(n: &Value) -> Option<PrHealth>`; `parse_pr` unchanged signature. `poll` skips session ids starting with `"review:"`.

- [ ] **Step 1: Write the failing test** (append inside `#[cfg(test)] mod tests` in github.rs)

```rust
    #[test]
    fn branch_query_unchanged_and_node_parse() {
        let old = "query($o:String!,$n:String!,$b:String!){viewer{login} repository(owner:$o,name:$n){pullRequests(headRefName:$b,first:1,orderBy:{field:CREATED_AT,direction:DESC},states:[OPEN,MERGED,CLOSED]){nodes{number url title state isDraft mergeable reviewDecision headRefOid baseRefName reviewThreads(first:100){nodes{id path line isResolved isOutdated comments(last:10){nodes{databaseId author{login} body}}}} latestReviews(first:20){nodes{state}} commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:100){nodes{__typename ... on CheckRun{name conclusion status databaseId checkSuite{workflowRun{databaseId}}} ... on StatusContext{context state}}}}}}}}}}}";
        assert_eq!(branch_query(), old);
        let n = json!({"number": 7, "url": "u", "title": "t", "state": "OPEN", "isDraft": true, "reviewThreads": {"nodes": []}, "commits": {"nodes": []}});
        let p = pr_node(&n).unwrap();
        assert_eq!((p.number, p.overall), (7, Overall::Draft));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd src-tauri && cargo test branch_query_unchanged_and_node_parse`
Expected: compile error, `branch_query` / `pr_node` not found.

- [ ] **Step 3: Implement**

Replace the `QUERY` const with:

```rust
/// GraphQL selection for one PR: shared by the per-branch poll and review mode.
pub const PR_FIELDS: &str = "number url title state isDraft mergeable reviewDecision headRefOid baseRefName reviewThreads(first:100){nodes{id path line isResolved isOutdated comments(last:10){nodes{databaseId author{login} body}}}} latestReviews(first:20){nodes{state}} commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:100){nodes{__typename ... on CheckRun{name conclusion status databaseId checkSuite{workflowRun{databaseId}}} ... on StatusContext{context state}}}}}}}";

fn branch_query() -> String {
    format!("query($o:String!,$n:String!,$b:String!){{viewer{{login}} repository(owner:$o,name:$n){{pullRequests(headRefName:$b,first:1,orderBy:{{field:CREATED_AT,direction:DESC}},states:[OPEN,MERGED,CLOSED]){{nodes{{{PR_FIELDS}}}}}}}}}")
}
```

`query()` becomes `gql(tok, &branch_query(), json!({"o": owner, "n": name, "b": branch}))`. Check for other users of `QUERY` with `grep -n "QUERY" src-tauri/src/*.rs` (tempering.rs may reference `github::QUERY`; if so switch it to `branch_query()` made `pub`).

Split `parse_pr`:

```rust
/// GraphQL response -> health of the newest PR for the branch (None = no PR).
pub fn parse_pr(json: &Value) -> Option<PrHealth> {
    json["data"]["repository"]["pullRequests"]["nodes"].get(0).filter(|n| n.is_object()).and_then(pr_node)
}

/// One PullRequest node (PR_FIELDS selection) -> health.
pub fn pr_node(n: &Value) -> Option<PrHealth> {
    // body of the old parse_pr from `let nodes = ...` to `Some(p)` unchanged
}
```

In `poll`, first line inside `for (id, s) in &sess {`:

```rust
        if id.starts_with("review:") {
            continue; // review-mode checkouts are polled by reviews.rs
        }
```

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test`
Expected: all pass (29 + 1 new).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/github.rs src-tauri/src/tempering.rs
git commit -m "github: share PR_FIELDS and per-node parser; skip review sessions in the task PR poll"
```

---

### Task 2: reviews.rs pure core (types, author resolution, buckets, merge, draft parse)

**Files:**
- Create: `src-tauri/src/reviews.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod reviews;`)

**Interfaces:**
- Consumes: `github::{PrHealth, Overall, PrState, pr_node}` (Task 1).
- Produces (all `pub`):
  - `Assignee { id: String, name: String, avatar_url: Option<String> }` (Clone, Debug, PartialEq)
  - `LinkedIssue { identifier: String, url: String }` (Serialize camelCase)
  - `Author` enum, serialized `{"kind":"linear","id","name","avatarUrl"}` / `{"kind":"unlinked","login"}`
  - `Bucket` enum: `NeedsReview, Draft, CiFailing, ChangesRequested, Approved` (camelCase)
  - `ReviewPr { key, session_id, owner, repo, number, base, body, branch, author_login, author, requested, issue: Option<LinkedIssue>, bucket, health: PrHealth }` (camelCase)
  - `Draft { summary, event, comments: Vec<DraftComment> }`, `DraftComment { path, line: u64, body }` (Serialize + Deserialize)
  - `fn bucket(o: Overall) -> Bucket`
  - `fn branch_ident(branch: &str) -> Option<String>`
  - `fn pr_url(url: &str) -> Option<(String, String, u64)>`
  - `fn session_id(owner: &str, repo: &str, n: u64) -> String` → `"review:<owner>/<repo>/<n>"`
  - `fn to_review(n: &Value, requested: bool) -> Option<ReviewPr>` (author = Unlinked(login), issue None)
  - `fn link(pr: &mut ReviewPr, issue: LinkedIssue, assignee: Option<Assignee>)`
  - `fn merge(requested: Vec<ReviewPr>, team: Vec<ReviewPr>) -> Vec<ReviewPr>`
  - `fn parse_draft(s: &str) -> Result<Draft, String>`

- [ ] **Step 1: Write the failing tests** (in `src-tauri/src/reviews.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(n: u64, login: &str, branch: &str) -> Value {
        json!({"number": n, "url": format!("https://github.com/o/r/pull/{n}"), "title": "t", "state": "OPEN", "isDraft": false,
               "reviewDecision": "REVIEW_REQUIRED", "reviewThreads": {"nodes": []}, "commits": {"nodes": []},
               "body": "b", "headRefName": branch, "baseRefName": "main", "author": {"login": login},
               "repository": {"name": "r", "owner": {"login": "o"}}})
    }
    fn amy() -> Assignee { Assignee { id: "u1".into(), name: "Amy".into(), avatar_url: None } }

    #[test]
    fn idents_and_urls() {
        assert_eq!(branch_ident("eng-123-fix-login").as_deref(), Some("ENG-123"));
        assert_eq!(branch_ident("zach/eng-42").as_deref(), Some("ENG-42"));
        assert_eq!(branch_ident("feature/fix-login"), None);
        assert_eq!(branch_ident("main"), None);
        assert_eq!(pr_url("https://github.com/o/r/pull/12"), Some(("o".into(), "r".into(), 12)));
        assert_eq!(pr_url("https://github.com/o/r/pull/12/files#diff"), Some(("o".into(), "r".into(), 12)));
        assert_eq!(pr_url("https://github.com/o/r/issues/12"), None);
        assert_eq!(pr_url("https://linear.app/x"), None);
        assert_eq!(session_id("o", "r", 5), "review:o/r/5");
    }

    #[test]
    fn buckets_follow_spec() {
        use Overall::*;
        for (o, b) in [(AwaitingReview, Bucket::NeedsReview), (CiPending, Bucket::NeedsReview), (UnresolvedComments, Bucket::NeedsReview),
                       (Draft, Bucket::Draft), (CiFailing, Bucket::CiFailing), (Conflict, Bucket::CiFailing),
                       (ChangesRequested, Bucket::ChangesRequested), (ReadyToMerge, Bucket::Approved)] {
            assert_eq!(bucket(o), b, "{o:?}");
        }
    }

    #[test]
    fn to_review_reads_node() {
        let p = to_review(&node(3, "bob", "eng-9-x"), true).unwrap();
        assert_eq!((p.key.as_str(), p.session_id.as_str(), p.base.as_str(), p.branch.as_str()), ("o/r#3", "review:o/r/3", "main", "eng-9-x"));
        assert_eq!(p.author, Author::Unlinked { login: "bob".into() });
        assert!(p.requested && p.issue.is_none());
        assert_eq!(p.bucket, Bucket::NeedsReview);
    }

    #[test]
    fn merge_dedupes_and_keeps_linear_author() {
        let req = to_review(&node(1, "bob", "x"), true).unwrap();
        let mut team = to_review(&node(1, "bob", "x"), false).unwrap();
        link(&mut team, LinkedIssue { identifier: "ENG-1".into(), url: "l".into() }, Some(amy()));
        let other = to_review(&node(2, "cat", "y"), false).unwrap();
        let m = merge(vec![req], vec![team, other]);
        assert_eq!(m.len(), 2);
        assert!(m[0].requested);
        assert_eq!(m[0].author, Author::Linear { id: "u1".into(), name: "Amy".into(), avatar_url: None });
        assert_eq!(m[0].issue.as_ref().unwrap().identifier, "ENG-1");
        assert_eq!(m[1].number, 2);
    }

    #[test]
    fn link_without_assignee_keeps_unlinked_author() {
        let mut p = to_review(&node(1, "bob", "x"), false).unwrap();
        link(&mut p, LinkedIssue { identifier: "ENG-1".into(), url: "l".into() }, None);
        assert_eq!(p.author, Author::Unlinked { login: "bob".into() });
        assert!(p.issue.is_some());
    }

    #[test]
    fn parse_draft_ok() {
        let d = parse_draft(r#"{"summary":"s","event":"APPROVE","comments":[{"path":"a.rs","line":3,"body":"nit"}]}"#).unwrap();
        assert_eq!((d.event.as_str(), d.comments.len(), d.comments[0].line), ("APPROVE", 1, 3));
        assert!(parse_draft(r#"{"summary":"s","event":"COMMENT"}"#).unwrap().comments.is_empty());
    }

    #[test]
    fn parse_draft_rejects_bad() {
        assert!(parse_draft(r#"{"summary":"s","event":"APPR"#).is_err()); // half-written
        assert!(parse_draft(r#"{"summary":"s","event":"MERGE"}"#).unwrap_err().contains("event"));
        assert!(parse_draft("").is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test reviews::`
Expected: compile errors (types/functions missing). Add `mod reviews;` to lib.rs first so the module compiles.

- [ ] **Step 3: Implement** (top of `src-tauri/src/reviews.rs`)

```rust
//! Review mode: other people's PRs (GitHub review requests + the team's Linear-linked PRs), authors as Linear users,
//! and an agent that drafts a review in a detached checkout. Nothing is posted until the user clicks.
use crate::github::{pr_node, Overall, PrHealth};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Assignee {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkedIssue {
    pub identifier: String,
    pub url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Author {
    Linear {
        id: String,
        name: String,
        #[serde(rename = "avatarUrl")]
        avatar_url: Option<String>,
    },
    Unlinked { login: String },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Bucket {
    NeedsReview,
    Draft,
    CiFailing,
    ChangesRequested,
    Approved,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPr {
    pub key: String,
    pub session_id: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub base: String,
    pub body: String,
    pub branch: String,
    pub author_login: String,
    pub author: Author,
    pub requested: bool,
    pub issue: Option<LinkedIssue>,
    pub bucket: Bucket,
    pub health: PrHealth,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DraftComment {
    pub path: String,
    pub line: u64,
    pub body: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Draft {
    pub summary: String,
    pub event: String,
    #[serde(default)]
    pub comments: Vec<DraftComment>,
}

pub fn bucket(o: Overall) -> Bucket {
    use Overall::*;
    match o {
        Draft => Bucket::Draft,
        CiFailing | Conflict => Bucket::CiFailing,
        ChangesRequested => Bucket::ChangesRequested,
        ReadyToMerge => Bucket::Approved,
        AwaitingReview | CiPending | UnresolvedComments | Merged | Closed => Bucket::NeedsReview, // merged/closed are filtered out before this
    }
}

/// Linear's branch format: `[user/]team-123-slug` -> "TEAM-123".
pub fn branch_ident(branch: &str) -> Option<String> {
    branch.split('/').find_map(|seg| {
        let (team, rest) = seg.split_once('-')?;
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let after = rest[num.len()..].chars().next();
        (!team.is_empty() && team.chars().all(|c| c.is_ascii_alphabetic()) && !num.is_empty() && matches!(after, None | Some('-')))
            .then(|| format!("{}-{num}", team.to_uppercase()))
    })
}

/// "https://github.com/<o>/<r>/pull/<n>[/...]" -> (o, r, n).
pub fn pr_url(url: &str) -> Option<(String, String, u64)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut it = rest.split(['/', '#', '?']);
    let (o, r, kind, n) = (it.next()?, it.next()?, it.next()?, it.next()?);
    (kind == "pull" && !o.is_empty() && !r.is_empty()).then_some(())?;
    Some((o.into(), r.into(), n.parse().ok()?))
}

pub fn session_id(owner: &str, repo: &str, n: u64) -> String {
    format!("review:{owner}/{repo}/{n}")
}

/// PullRequest node (PR_FIELDS + body headRefName author repository) -> row, author unlinked until `link`.
pub fn to_review(n: &Value, requested: bool) -> Option<ReviewPr> {
    let health = pr_node(n)?;
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let (owner, repo) = (s(&n["repository"]["owner"]["login"]), s(&n["repository"]["name"]));
    let login = s(&n["author"]["login"]);
    Some(ReviewPr {
        key: format!("{owner}/{repo}#{}", health.number),
        session_id: session_id(&owner, &repo, health.number),
        number: health.number,
        base: s(&n["baseRefName"]),
        body: s(&n["body"]),
        branch: s(&n["headRefName"]),
        author: Author::Unlinked { login: login.clone() },
        author_login: login,
        requested,
        issue: None,
        bucket: bucket(health.overall),
        owner,
        repo,
        health,
    })
}

pub fn link(pr: &mut ReviewPr, issue: LinkedIssue, assignee: Option<Assignee>) {
    pr.issue = Some(issue);
    if let Some(a) = assignee {
        pr.author = Author::Linear { id: a.id, name: a.name, avatar_url: a.avatar_url };
    }
}

/// Union by URL. A PR in both keeps `requested` and the team copy's Linear author/issue. Requested first.
pub fn merge(requested: Vec<ReviewPr>, team: Vec<ReviewPr>) -> Vec<ReviewPr> {
    let mut out = requested;
    for t in team {
        match out.iter_mut().find(|p| p.health.url == t.health.url) {
            Some(p) => {
                if p.issue.is_none() {
                    p.issue = t.issue;
                }
                if matches!(p.author, Author::Unlinked { .. }) {
                    p.author = t.author;
                }
            }
            None => out.push(t),
        }
    }
    out.sort_by(|a, b| b.requested.cmp(&a.requested).then(a.key.cmp(&b.key)));
    out
}

pub fn parse_draft(s: &str) -> Result<Draft, String> {
    let d: Draft = serde_json::from_str(s).map_err(|e| format!("review.json: {e}"))?;
    if !matches!(d.event.as_str(), "COMMENT" | "APPROVE" | "REQUEST_CHANGES") {
        return Err(format!("review.json: event must be COMMENT, APPROVE or REQUEST_CHANGES, got {}", d.event));
    }
    Ok(d)
}
```

`github::Overall` and `PrHealth` must be `Debug + Clone + PartialEq` (they already are). If `Overall` is not `pub` re-exported, it already is `pub enum`.

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test`
Expected: all pass. (Expect dead-code warnings until Task 3 uses these; add `#![allow(dead_code)]` at the top of reviews.rs only if warnings fail the build — they don't by default; remove it in Task 3.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/reviews.rs src-tauri/src/lib.rs
git commit -m "reviews: PR rows, Linear author resolution, status buckets, draft parsing"
```

---

### Task 3: Fetching + poller + commands + remembered filters

**Files:**
- Modify: `src-tauri/src/linear.rs` (queries + parsers + `api_key()` helper)
- Modify: `src-tauri/src/reviews.rs` (io, state, poller, commands)
- Modify: `src-tauri/src/settings.rs` (`review_filters`, `set_review_filters`, `save_settings` preserves it)
- Modify: `src-tauri/src/lib.rs` (manage state, start poller, register commands)

**Interfaces:**
- Consumes: Task 2 types; `github::{token, gql, PR_FIELDS}`.
- Produces:
  - `linear::team_pr_links(key: &str) -> Result<Vec<(String /*pr url*/, LinkedIssue, Option<Assignee>)>, String>`
  - `linear::issue_by_ident(key: &str, ident: &str) -> Result<Option<(LinkedIssue, Option<Assignee>)>, String>`
  - `reviews::Payload { prs: Vec<ReviewPr>, error: Option<String> }` (Serialize camelCase, Clone, PartialEq, Default)
  - `reviews::Reviews` state (Default); `reviews::start(app)`; event `"reviews-updated"` with `Payload`
  - `reviews::combine(gh: Result<(String, Vec<ReviewPr>), String>, team: Result<Vec<ReviewPr>, String>) -> Payload`
  - Commands: `get_reviews() -> Payload`, `refresh_reviews()`, `settings::set_review_filters(filters: ReviewFilters)`
  - `settings::ReviewFilters { authors: Vec<String>, statuses: Vec<String> }` (snake_case wire, Default), `Settings.review_filters`

- [ ] **Step 1: Write the failing tests**

In `linear.rs` tests:

```rust
    #[test]
    fn team_links_keep_only_pr_attachments() {
        let j = json!({"data": {"issues": {"nodes": [
            {"identifier": "ENG-1", "url": "l1", "assignee": {"id": "u1", "name": "Amy", "avatarUrl": "https://a"},
             "attachments": {"nodes": [{"url": "https://github.com/o/r/pull/4"}, {"url": "https://figma.com/x"}]}},
            {"identifier": "ENG-2", "url": "l2", "assignee": null, "attachments": {"nodes": []}}
        ]}}});
        let v = parse_team_links(&j).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "https://github.com/o/r/pull/4");
        assert_eq!(v[0].1.identifier, "ENG-1");
        assert_eq!(v[0].2.as_ref().unwrap().avatar_url.as_deref(), Some("https://a"));
        assert!(parse_team_links(&json!({"errors": [{"message": "bad"}]})).is_err());
    }

    #[test]
    fn issue_by_ident_parse() {
        let j = json!({"data": {"issue": {"identifier": "ENG-9", "url": "l", "assignee": {"id": "u", "name": "Bo", "avatarUrl": null}}}});
        let (i, a) = parse_ident(&j).unwrap().unwrap();
        assert_eq!((i.identifier.as_str(), a.unwrap().name.as_str()), ("ENG-9", "Bo"));
        assert!(parse_ident(&json!({"data": {"issue": null}})).unwrap().is_none());
        // Linear answers an unknown identifier with an "Entity not found" error: treat as no issue
        assert!(parse_ident(&json!({"errors": [{"message": "Entity not found: Issue"}]})).unwrap().is_none());
    }
```

In `reviews.rs` tests:

```rust
    #[test]
    fn combine_partial_failure() {
        let pr = to_review(&node(1, "bob", "x"), true).unwrap();
        let p = combine(Ok(("me".into(), vec![pr.clone()])), Err("Linear: no key".into()));
        assert_eq!((p.prs.len(), p.error.as_deref()), (1, Some("Linear: no key")));
        let p = combine(Err("Not logged in to GitHub".into()), Ok(vec![]));
        assert!(p.prs.is_empty() && p.error.unwrap().contains("GitHub"));
        let mine = to_review(&node(2, "me", "y"), false).unwrap();
        let p = combine(Ok(("me".into(), vec![pr])), Ok(vec![mine])); // own PR dropped
        assert_eq!(p.prs.len(), 1);
    }
```

In `settings.rs` `round_trip_and_defaults`, add `review_filters: ReviewFilters { authors: vec!["u1".into(), "unlinked".into()], statuses: vec!["draft".into()] }` to the `Settings { .. }` literal.

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test`
Expected: compile errors for `parse_team_links`, `parse_ident`, `combine`, `ReviewFilters`.

- [ ] **Step 3: Implement**

`linear.rs` (reuse the module's `post`; add an errors helper so both parsers share it):

```rust
use crate::reviews::{pr_url, Assignee, LinkedIssue};

const MY_TEAMS: &str = r#"query { viewer { teams { nodes { id } } } }"#;
const TEAM_PRS: &str = r#"query($teams: [ID!]) { issues(first: 100, filter: { team: { id: { in: $teams } }, state: { type: { eq: "started" } } }) { nodes { identifier url assignee { id name avatarUrl } attachments { nodes { url } } } } }"#;
const BY_IDENT: &str = r#"query($id: String!) { issue(id: $id) { identifier url assignee { id name avatarUrl } } }"#;

pub fn api_key() -> Result<String, String> {
    crate::settings::entry()?.get_password().map_err(|_| "Add your Linear API key in Settings".to_string())
}

fn gql_err(json: &Value) -> Option<String> {
    let errs = json.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty())?;
    Some(format!("Linear: {}", errs.iter().filter_map(|e| e["message"].as_str()).collect::<Vec<_>>().join("; ")))
}

fn assignee(v: &Value) -> Option<Assignee> {
    Some(Assignee { id: v["id"].as_str()?.into(), name: v["name"].as_str().unwrap_or_default().into(), avatar_url: v["avatarUrl"].as_str().map(String::from) })
}

fn linked(n: &Value) -> LinkedIssue {
    LinkedIssue { identifier: n["identifier"].as_str().unwrap_or_default().into(), url: n["url"].as_str().unwrap_or_default().into() }
}

fn parse_team_links(json: &Value) -> Result<Vec<(String, LinkedIssue, Option<Assignee>)>, String> {
    if let Some(e) = gql_err(json) {
        return Err(e);
    }
    let mut out = vec![];
    for n in json["data"]["issues"]["nodes"].as_array().ok_or("Linear: unexpected response")? {
        for a in n["attachments"]["nodes"].as_array().into_iter().flatten() {
            let url = a["url"].as_str().unwrap_or_default();
            if pr_url(url).is_some() {
                out.push((url.to_string(), linked(n), assignee(&n["assignee"])));
            }
        }
    }
    Ok(out)
}

fn parse_ident(json: &Value) -> Result<Option<(LinkedIssue, Option<Assignee>)>, String> {
    match gql_err(json) {
        Some(e) if e.contains("not found") => return Ok(None),
        Some(e) => return Err(e),
        None => {}
    }
    let n = &json["data"]["issue"];
    Ok((!n.is_null()).then(|| (linked(n), assignee(&n["assignee"]))))
}

/// PRs attached to started issues in the viewer's teams. ponytail: first 100 issues; page if teams outgrow it.
pub fn team_pr_links(key: &str) -> Result<Vec<(String, LinkedIssue, Option<Assignee>)>, String> {
    let t = post(key, json!({ "query": MY_TEAMS }))?;
    if let Some(e) = gql_err(&t) {
        return Err(e);
    }
    let teams: Vec<&str> = t["data"]["viewer"]["teams"]["nodes"].as_array().into_iter().flatten().filter_map(|n| n["id"].as_str()).collect();
    parse_team_links(&post(key, json!({ "query": TEAM_PRS, "variables": { "teams": teams } }))?)
}

pub fn issue_by_ident(key: &str, ident: &str) -> Result<Option<(LinkedIssue, Option<Assignee>)>, String> {
    parse_ident(&post(key, json!({ "query": BY_IDENT, "variables": { "id": ident } }))?)
}
```

Make `get_issue_detail` use `api_key()` (same message) to avoid duplication.

`settings.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct ReviewFilters {
    /// Linear user ids, or "unlinked"
    pub authors: Vec<String>,
    /// Bucket names: needsReview, draft, ciFailing, changesRequested, approved
    pub statuses: Vec<String>,
}
```

Add `pub review_filters: ReviewFilters,` to `Settings`. In `save_settings`, next to the caffeine line: `settings.review_filters = load(&d).review_filters; // owned by the Reviews filter bar`. New command:

```rust
#[tauri::command]
pub fn set_review_filters(app: AppHandle, filters: ReviewFilters) -> Result<(), String> {
    let d = dir(&app)?;
    let mut s = load(&d);
    s.review_filters = filters;
    save(&d, &s)
}
```

`reviews.rs` io (append above tests):

```rust
use crate::{github, linear};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Serialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub prs: Vec<ReviewPr>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Reviews {
    payload: Mutex<Payload>,
    /// identifier -> Linear issue (None = no such issue); per app run
    idents: Mutex<HashMap<String, Option<(LinkedIssue, Option<Assignee>)>>>,
    wake: (Mutex<bool>, Condvar),
}

fn search_query() -> String {
    format!("query($q:String!){{viewer{{login}} search(query:$q,type:ISSUE,first:50){{nodes{{... on PullRequest{{{} body headRefName author{{login}} repository{{name owner{{login}}}}}}}}}}}}", github::PR_FIELDS)
}

fn url_query() -> String {
    format!("query($u:URI!){{resource(url:$u){{... on PullRequest{{{} body headRefName author{{login}} repository{{name owner{{login}}}}}}}}}}", github::PR_FIELDS)
}

/// (viewer login, PRs where GitHub asks the viewer or one of their teams to review).
fn requested(tok: &str) -> Result<(String, Vec<ReviewPr>), String> {
    let j = github::gql(tok, &search_query(), json!({"q": "is:pr is:open review-requested:@me archived:false"}))?;
    let me = j["data"]["viewer"]["login"].as_str().unwrap_or_default().to_string();
    Ok((me, j["data"]["search"]["nodes"].as_array().into_iter().flatten().filter_map(|n| to_review(n, true)).collect()))
}

/// The team's Linear-linked PRs, minus URLs already fetched via search. ponytail: one GitHub query per PR; alias-batch if slow.
fn team(tok: &str, skip: &[String]) -> Result<Vec<ReviewPr>, String> {
    let key = linear::api_key()?;
    let mut out: Vec<ReviewPr> = vec![];
    for (url, issue, who) in linear::team_pr_links(&key)? {
        if skip.contains(&url) || out.iter().any(|p| p.health.url == url) {
            continue;
        }
        let j = github::gql(tok, &url_query(), json!({"u": url}))?;
        if let Some(mut p) = to_review(&j["data"]["resource"], false) {
            link(&mut p, issue, who);
            out.push(p);
        }
    }
    Ok(out)
}

/// Branch-name fallback for rows still unlinked (cached per run).
fn link_by_branch(app: &AppHandle, prs: &mut [ReviewPr]) {
    let Ok(key) = linear::api_key() else { return };
    let st = app.state::<Reviews>();
    for p in prs.iter_mut().filter(|p| p.issue.is_none()) {
        let Some(id) = branch_ident(&p.branch) else { continue };
        let hit = st.idents.lock().unwrap().get(&id).cloned();
        let hit = match hit {
            Some(h) => h,
            None => match linear::issue_by_ident(&key, &id) {
                Ok(h) => {
                    st.idents.lock().unwrap().insert(id, h.clone());
                    h
                }
                Err(_) => continue, // transient: retry next poll
            },
        };
        if let Some((issue, who)) = hit {
            link(p, issue, who);
        }
    }
}

/// Merge both sources; either may fail without hiding the other. Drops the viewer's own and non-open PRs.
pub fn combine(gh: Result<(String, Vec<ReviewPr>), String>, team: Result<Vec<ReviewPr>, String>) -> Payload {
    let errs: Vec<String> = [gh.as_ref().err(), team.as_ref().err()].into_iter().flatten().cloned().collect();
    let (me, req) = gh.unwrap_or_default();
    let mut prs = merge(req, team.unwrap_or_default());
    prs.retain(|p| p.author_login != me && p.health.state == github::PrState::Open);
    Payload { prs, error: (!errs.is_empty()).then(|| errs.join("; ")) }
}

fn poll(app: &AppHandle) {
    let p = match github::token() {
        Err(e) => Payload { prs: vec![], error: Some(e) },
        Ok(tok) => {
            let gh = requested(&tok);
            let skip: Vec<String> = gh.as_ref().map(|(_, v)| v.iter().map(|p| p.health.url.clone()).collect()).unwrap_or_default();
            let t = team(&tok, &skip);
            let mut p = combine(gh, t);
            link_by_branch(app, &mut p.prs);
            p
        }
    };
    let st = app.state::<Reviews>();
    let mut cur = st.payload.lock().unwrap();
    if *cur != p {
        *cur = p;
        let _ = app.emit("reviews-updated", &*cur);
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        poll(&app);
        let (m, cv) = &app.state::<Reviews>().wake;
        drop(cv.wait_timeout_while(m.lock().unwrap(), Duration::from_secs(120), |woken| !*woken).unwrap().0);
        *m.lock().unwrap() = false;
    });
}

#[tauri::command]
pub fn get_reviews(state: State<Reviews>) -> Payload {
    state.payload.lock().unwrap().clone()
}

#[tauri::command]
pub fn refresh_reviews(state: State<Reviews>) {
    *state.wake.0.lock().unwrap() = true;
    state.wake.1.notify_one();
}
```

Make `github::PrState` usable (`pub`, already). `combine` needs `Default` for `(String, Vec<ReviewPr>)` — tuples of Default types implement Default.

`lib.rs`: `.manage(reviews::Reviews::default())`, `reviews::start(app.handle().clone());` in setup after `github::start`, and register `reviews::get_reviews, reviews::refresh_reviews, settings::set_review_filters` in `generate_handler!`.

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test && cargo build`
Expected: all pass; build has no warnings from reviews.rs (remove any `allow(dead_code)` added in Task 2).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/linear.rs src-tauri/src/reviews.rs src-tauri/src/settings.rs src-tauri/src/lib.rs
git commit -m "reviews: poll GitHub review requests + team Linear PRs, link authors, remembered filters"
```

---

### Task 4: Review checkout, agent session, draft, post, dismiss, diff, cleanup

**Files:**
- Modify: `src-tauri/src/reviews.rs`
- Modify: `src-tauri/src/sessions.rs` (make `write_hooks` `pub(crate)`; add `start_review`, `forget`)
- Modify: `src-tauri/src/lib.rs` (register commands)

**Interfaces:**
- Consumes: Task 2/3 (`ReviewPr`, `Reviews`, `parse_draft`, `Draft`); `sessions::{git, spawn, write_hooks, exclude_settings, update/SessionInfo}`; `github::parse_remote`; `repo_match::list_repos`; `tempering::gh`.
- Produces:
  - `fn find_clone(root: &Path, owner: &str, repo: &str) -> Option<PathBuf>`
  - `fn ensure_review_worktree(home: &Path, clone: &Path, repo: &str, n: u64) -> Result<PathBuf, String>` → `~/.murim/reviews/<repo>/<n>`, detached at `refs/murim/pr-<n>`
  - `fn stale(session_ids: &[String], open: &[ReviewPr]) -> Vec<String>`
  - `sessions::start_review(app: &AppHandle, id: &str, wt: &str, prompt: String) -> Result<(), String>`
  - `sessions::forget(app: &AppHandle, id: &str)` (kill PTY, drop session)
  - Commands (all take `key: String` = `ReviewPr.key`, e.g. `"o/r#3"`):
    - `start_review_agent(app, key) -> Result<String /*session id*/, String>`
    - `get_review_draft(app, key) -> Result<Option<Draft>, String>` (Err text includes the raw file)
    - `post_review(app, key, draft: Draft) -> Result<(), String>`
    - `dismiss_review(app, key) -> Result<(), String>`
    - `get_pr_diff(app, key) -> Result<PrDiff, String>`, `PrDiff { diff: String, truncated: bool }` (camelCase)

- [ ] **Step 1: Write the failing tests** (reviews.rs tests)

```rust
    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    #[test]
    fn review_worktree_tracks_pull_ref() {
        let root = std::env::temp_dir().join(format!("murim-rev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let up = root.join("up");
        std::fs::create_dir_all(&up).unwrap();
        git(&up, &["init", "-q", "-b", "main"]);
        git(&up, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "base"]);
        git(&up, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "pr1"]);
        git(&up, &["update-ref", "refs/pull/1/head", "HEAD"]);
        git(&root, &["clone", "-q", up.to_str().unwrap(), "clone"]);
        let clone = root.join("clone");
        let wt = ensure_review_worktree(&root, &clone, "clone", 1).unwrap();
        assert_eq!(wt, root.join(".murim/reviews/clone/1"));
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), git(&up, &["rev-parse", "refs/pull/1/head"]));
        // PR gets a new commit: re-running moves the checkout
        git(&up, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "pr1b"]);
        git(&up, &["update-ref", "refs/pull/1/head", "HEAD"]);
        ensure_review_worktree(&root, &clone, "clone", 1).unwrap();
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), git(&up, &["rev-parse", "refs/pull/1/head"]));
        remove_review_worktree(&clone, &wt, 1).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_clone_matches_origin() {
        let root = std::env::temp_dir().join(format!("murim-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let r = root.join("whatever-name");
        std::fs::create_dir_all(&r).unwrap();
        git(&r, &["init", "-q"]);
        git(&r, &["remote", "add", "origin", "git@github.com:Acme/Widget.git"]);
        assert_eq!(find_clone(&root, "acme", "widget"), Some(r.clone())); // case-insensitive, dir name irrelevant
        assert_eq!(find_clone(&root, "acme", "other"), None);
        assert_eq!(find_clone(&root.join("missing"), "acme", "widget"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_only_after_success() {
        let open = vec![to_review(&node(1, "bob", "x"), true).unwrap()];
        let ids = vec!["review:o/r/1".to_string(), "review:o/r/2".to_string(), "issue-abc".to_string()];
        assert_eq!(stale(&ids, &open), vec!["review:o/r/2".to_string()]);
    }
```

(The "only after success" half is enforced in `poll`: call `stale` only when `p.error.is_none()`. Keep that `if` adjacent to the call with a comment.)

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test reviews::`
Expected: compile errors for `ensure_review_worktree`, `remove_review_worktree`, `find_clone`, `stale`.

- [ ] **Step 3: Implement**

`reviews.rs`:

```rust
use crate::sessions::{self, git};
use std::path::{Path, PathBuf};

/// Clone under repos_root whose origin is github.com/<owner>/<repo> (dir name doesn't matter).
pub fn find_clone(root: &Path, owner: &str, repo: &str) -> Option<PathBuf> {
    crate::repo_match::list_repos(root).into_iter().map(|r| PathBuf::from(r.path)).find(|p| {
        git(p, &["remote", "get-url", "origin"]).ok().and_then(|u| github::parse_remote(&u))
            .is_some_and(|(o, r)| o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo))
    })
}

/// Detached checkout of the PR head at ~/.murim/reviews/<repo>/<n>; re-running moves it to the latest head.
pub fn ensure_review_worktree(home: &Path, clone: &Path, repo: &str, n: u64) -> Result<PathBuf, String> {
    let r = format!("refs/murim/pr-{n}");
    git(clone, &["fetch", "-q", "origin", &format!("+refs/pull/{n}/head:{r}")])?;
    let _ = git(clone, &["fetch", "-q", "origin"]); // base branch for `git diff origin/<base>...HEAD`; best effort
    let wt = home.join(".murim/reviews").join(repo).join(n.to_string());
    if wt.exists() {
        git(&wt, &["checkout", "-q", "--detach", "--force", &r])?;
    } else {
        std::fs::create_dir_all(wt.parent().unwrap()).map_err(|e| e.to_string())?;
        git(clone, &["worktree", "add", "-q", "--detach", &wt.to_string_lossy(), &r])?;
    }
    Ok(wt)
}

pub fn remove_review_worktree(clone: &Path, wt: &Path, n: u64) -> Result<(), String> {
    if wt.exists() {
        git(clone, &["worktree", "remove", "--force", &wt.to_string_lossy()])?;
    }
    let _ = git(clone, &["update-ref", "-d", &format!("refs/murim/pr-{n}")]);
    Ok(())
}

/// Review sessions whose PR is no longer in the (successfully fetched) open list.
pub fn stale(session_ids: &[String], open: &[ReviewPr]) -> Vec<String> {
    session_ids.iter().filter(|id| id.starts_with("review:") && !open.iter().any(|p| &p.session_id == *id)).cloned().collect()
}

fn review_prompt(p: &ReviewPr) -> String {
    let who = match &p.author { Author::Linear { name, .. } => name.clone(), Author::Unlinked { login } => login.clone() };
    format!("You are reviewing pull request {url} \"{title}\" by {who}. Its code is checked out here (detached HEAD); the base branch is origin/{base}. Read the change with `git diff origin/{base}...HEAD` and the surrounding code.\n\nDo NOT post to GitHub, comment, commit or push. Murim posts the review after the user approves it.\n\nWrite your review to .murim/review.json as JSON: {{\"summary\": string (markdown), \"event\": \"COMMENT\" | \"APPROVE\" | \"REQUEST_CHANGES\", \"comments\": [{{\"path\": string, \"line\": number (line in the new version of the file; must be inside the diff), \"body\": string}}]}}. Write the file in one go, then say it is ready.\n\nPR description:\n{body}",
        url = p.health.url, title = p.health.title, base = p.base, body = p.body)
}

fn find(app: &AppHandle, key: &str) -> Result<ReviewPr, String> {
    app.state::<Reviews>().payload.lock().unwrap().prs.iter().find(|p| p.key == key).cloned().ok_or_else(|| format!("{key} is no longer in the review list"))
}

fn home() -> Result<PathBuf, String> {
    std::env::var("HOME").map(PathBuf::from).map_err(|e| e.to_string())
}

fn wt_of(p: &ReviewPr) -> Result<PathBuf, String> {
    Ok(home()?.join(".murim/reviews").join(&p.repo).join(p.number.to_string()))
}

fn clone_of(app: &AppHandle, p: &ReviewPr) -> Result<PathBuf, String> {
    let root = crate::settings::load(&crate::settings::dir(app)?).repos_root;
    find_clone(Path::new(&root), &p.owner, &p.repo).ok_or_else(|| format!("Clone {}/{} under your repos folder first", p.owner, p.repo))
}

#[tauri::command]
pub fn start_review_agent(app: AppHandle, key: String) -> Result<String, String> {
    let p = find(&app, &key)?;
    let clone = clone_of(&app, &p)?;
    let wt = ensure_review_worktree(&home()?, &clone, &p.repo, p.number)?;
    sessions::exclude_settings(&clone)?;
    sessions::write_hooks(&wt)?;
    std::fs::create_dir_all(wt.join(".murim")).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(wt.join(".murim/review.json")); // a re-run drafts afresh
    sessions::start_review(&app, &p.session_id, &wt.to_string_lossy(), review_prompt(&p))?;
    Ok(p.session_id)
}

#[tauri::command]
pub fn get_review_draft(app: AppHandle, key: String) -> Result<Option<Draft>, String> {
    let f = wt_of(&find(&app, &key)?)?.join(".murim/review.json");
    match std::fs::read_to_string(&f) {
        Err(_) => Ok(None),
        Ok(s) => parse_draft(&s).map(Some).map_err(|e| format!("{e}\n\n{s}")),
    }
}

#[tauri::command]
pub fn post_review(app: AppHandle, key: String, draft: Draft) -> Result<(), String> {
    let p = find(&app, &key)?;
    let draft = parse_draft(&serde_json::to_string(&draft).map_err(|e| e.to_string())?)?; // same event validation as the file
    let body = json!({
        "body": draft.summary,
        "event": draft.event,
        "comments": draft.comments.iter().map(|c| json!({"path": c.path, "line": c.line, "side": "RIGHT", "body": c.body})).collect::<Vec<_>>(),
    });
    let f = std::env::temp_dir().join(format!("murim-review-{}-{}.json", p.repo, p.number));
    std::fs::write(&f, body.to_string()).map_err(|e| e.to_string())?;
    let r = crate::tempering::gh(&home()?.to_string_lossy(), &["api", "-X", "POST", &format!("repos/{}/{}/pulls/{}/reviews", p.owner, p.repo, p.number), "--input", &f.to_string_lossy()]);
    let _ = std::fs::remove_file(&f);
    r?;
    refresh_reviews(app.state());
    Ok(())
}

#[tauri::command]
pub fn dismiss_review(app: AppHandle, key: String) -> Result<(), String> {
    let p = find(&app, &key)?;
    sessions::forget(&app, &p.session_id);
    remove_review_worktree(&clone_of(&app, &p)?, &wt_of(&p)?, p.number)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDiff {
    diff: String,
    truncated: bool,
}

#[tauri::command]
pub async fn get_pr_diff(app: AppHandle, key: String) -> Result<PrDiff, String> {
    let url = find(&app, &key)?.health.url;
    // gh is blocking and can be slow on big PRs: keep it off the async runtime
    let mut diff = tauri::async_runtime::spawn_blocking(move || crate::tempering::gh(&home()?.to_string_lossy(), &["pr", "diff", &url, "--color", "never"]))
        .await
        .map_err(|e| e.to_string())??;
    const MAX_DIFF: usize = 1_500_000; // same cap as review.rs
    let truncated = diff.len() > MAX_DIFF;
    if truncated {
        let mut cut = MAX_DIFF;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
    }
    Ok(PrDiff { diff, truncated })
}
```

In `poll`, after computing `p` and before publishing:

```rust
    // only a successful fetch can say a PR closed; offline must not wipe review checkouts
    if p.error.is_none() {
        let root = crate::settings::dir(app).map(|d| crate::settings::load(&d).repos_root).unwrap_or_default();
        let ids: Vec<String> = sessions::get_sessions(app.state()).into_keys().collect();
        for id in stale(&ids, &p.prs) {
            sessions::forget(app, &id);
            if let (Some((o, r, n)), Ok(h)) = (parse_session(&id), home()) {
                if let Some(c) = find_clone(Path::new(&root), &o, &r) {
                    let _ = remove_review_worktree(&c, &h.join(".murim/reviews").join(&r).join(n.to_string()), n);
                }
            }
        }
    }
```

with

```rust
/// "review:<owner>/<repo>/<n>" -> parts.
fn parse_session(id: &str) -> Option<(String, String, u64)> {
    let mut it = id.strip_prefix("review:")?.splitn(3, '/');
    Some((it.next()?.into(), it.next()?.into(), it.next()?.parse().ok()?))
}
```

Add to the `idents_and_urls` test: `assert_eq!(parse_session("review:o/r/5"), Some(("o".into(), "r".into(), 5)));` and `assert_eq!(parse_session("issue-1"), None);`.

`sessions.rs` — change `fn write_hooks` to `pub(crate) fn write_hooks`, and add:

```rust
/// Review-mode agent in a PR checkout. Same PTY/hook machinery as tasks, keyed `review:<owner>/<repo>/<n>`; no PR flow, no Tempering.
pub fn start_review(app: &AppHandle, id: &str, wt: &str, prompt: String) -> Result<(), String> {
    let cfg = settings::load(&settings::dir(app)?);
    if cfg.agent == AgentKind::Custom && cfg.agent_command.trim().is_empty() {
        return Err("Set the agent command in Settings".into());
    }
    update(app, |g| {
        if g.ptys.contains_key(id) {
            return Err("already running".to_string());
        }
        g.map.insert(id.into(), SessionInfo { issue_id: id.into(), worktree: wt.into(), session_id: None, status: Status::Starting, questions: None, pty_alive: false, permission_mode: cfg.default_permission_mode, tempering: false, escalations: vec![], memory: Default::default(), pr_flow: PrFlow::None, agent: cfg.agent, agent_command: cfg.agent_command.trim().to_string(), tempering_turn: false });
        Ok(())
    })?;
    spawn(app, id, vec![prompt])
}

/// Kill a session's PTY and drop it (review dismissed / PR closed).
pub fn forget(app: &AppHandle, id: &str) {
    update(app, |g| {
        if let Some(mut p) = g.ptys.remove(id) {
            let _ = p.child.kill();
        }
        g.map.remove(id);
        g.bufs.remove(id);
    });
}
```

(Check `PrFlow`'s default variant name in hooks.rs — the summary calls it `None`; use `Default::default()` if unsure. Check `Memory: Default`.)

Verify hooks on Stop do nothing harmful for review sessions: in `sessions::on_hook`, the quality pass only triggers on `MURIM_DONE` and repo-pick only when `worktree` is empty; review sessions have a worktree and never print those markers. `tempering::consume` must no-op when `tempering == false` — read it and confirm; if it doesn't, add `if id.starts_with("review:") { return; }` at its top.

`lib.rs`: register `reviews::start_review_agent, reviews::get_review_draft, reviews::post_review, reviews::dismiss_review, reviews::get_pr_diff`.

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test && cargo build`
Expected: all pass, no warnings.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/reviews.rs src-tauri/src/sessions.rs src-tauri/src/lib.rs src-tauri/src/tempering.rs
git commit -m "reviews: PR checkout + agent-drafted review, post on click, dismiss, diff, cleanup on close"
```

---

### Task 5: Icon rail and two-mode layout

**Files:**
- Modify: `src/App.tsx`, `src/App.css`, `src/Icons.tsx`, `src/Updater.tsx`

**Interfaces:**
- Produces: `App` state `view: "tasks" | "reviews"` (remembered in `localStorage["murim.view"]`, wrapped in try/catch), `openReview: string | null` (a `ReviewPr.key`). Icons `TasksIcon`, `ReviewsIcon`, `DownloadIcon` (same unboxed stroke style as `GearIcon`). In reviews mode the sidebar renders a `.section-label` "Reviews" header and the main pane an empty state "Select a pull request to review." (Tasks 6–7 replace these with real components.)

- [ ] **Step 1: Implement the rail**

In `App.tsx`, wrap the current `<aside className="sidebar">` with a rail before it:

```tsx
<nav className="rail" data-tauri-drag-region>
  <button className={`btn icon ${view === "tasks" ? "on" : ""}`} title="Tasks" onClick={() => switchView("tasks")}><TasksIcon /></button>
  <button className={`btn icon ${view === "reviews" ? "on" : ""}`} title="Reviews" onClick={() => switchView("reviews")}><ReviewsIcon /></button>
  <div className="rail-bottom">
    <Updater />
    <Caffeine />
    <button className={`btn icon ${showSettings ? "on" : ""}`} title="Settings" onClick={() => setShowSettings(!showSettings)}><GearIcon /></button>
  </div>
</nav>
```

`switchView(v)` sets `view`, closes Settings, saves to localStorage. Remove the `sidebar-footer` (its contents moved to the rail). Keep `brand` header in the sidebar for both modes (logo + refresh button; refresh calls `refresh_inbox` in tasks mode, `refresh_reviews` in reviews mode).

`Updater.tsx`: render an icon button instead of text — `<button className="btn icon update" title={label} onClick={install}><DownloadIcon /></button>` where `label` is the current text ("Update to vX.Y.Z" / "Downloading N%" / "Installing…" / error). Add a small brand-colored dot via CSS `.update::after` so an available update is visible.

CSS (`App.css`): 

```css
/* icon rail: macOS traffic lights sit above its icons (window has no title bar) */
.rail { width: 52px; flex: none; display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 52px 0 10px; background: var(--surface); border-right: 1px solid var(--line); }
.rail .btn.icon { width: 34px; height: 34px; background: none; border-color: transparent; box-shadow: none; color: var(--dim); }
.rail .btn.icon:hover { color: var(--text); background: var(--hover); }
.rail .btn.icon.on { color: var(--brand); background: var(--hover); }
.rail-bottom { margin-top: auto; display: flex; flex-direction: column; align-items: center; gap: 4px; }
.update { position: relative; }
.update::after { content: ""; position: absolute; top: 6px; right: 6px; width: 6px; height: 6px; border-radius: 50%; background: var(--brand); }
```

Change `.brand` left padding from `80px` to `28px` (traffic lights at x 14–66 now overlap rail + 14px of sidebar; 52 + 28 = 80). Move the `.sidebar-footer .btn.icon` rules to `.rail` (delete the old ones). Check `Caffeine.tsx` renders an icon button that fits (34px).

- [ ] **Step 2: Verify**

Run: `pnpm exec tsc --noEmit` → clean.
Headless check: build (`pnpm build`) and render `dist/index.html` with the existing scratchpad mock approach (`__TAURI_INTERNALS__` stub returning empty data) in headless Chrome at 1100×720; confirm the rail shows two top icons and three bottom icons, the Tasks list still renders, clicking Reviews shows the Reviews header and empty state. If no mock is available, state that the UI was verified by type-check only.

- [ ] **Step 3: Commit**

```bash
git add src/App.tsx src/App.css src/Icons.tsx src/Updater.tsx
git commit -m "Icon rail: switch Tasks / Reviews; settings, caffeine and updater move to the rail"
```

---

### Task 6: Review list with Linear-style filters

**Files:**
- Create: `src/reviewFilter.ts`, `scripts/reviewFilter.check.ts`, `src/ReviewList.tsx`
- Modify: `src/types.ts` (ReviewPr types), `src/App.tsx` (render `ReviewList` in reviews mode; load `get_reviews` + listen `reviews-updated`), `src/App.css`

**Interfaces:**
- Consumes: commands `get_reviews`, `refresh_reviews`, `set_review_filters`, `get_settings` (`review_filters: { authors, statuses }`), event `reviews-updated` (`{ prs: ReviewPr[], error: string | null }`).
- Produces (in `src/types.ts`):

```ts
export type Bucket = "needsReview" | "draft" | "ciFailing" | "changesRequested" | "approved";
export type ReviewAuthor = { kind: "linear"; id: string; name: string; avatarUrl: string | null } | { kind: "unlinked"; login: string };
export type ReviewPr = {
  key: string; sessionId: string; owner: string; repo: string; number: number; base: string; body: string; branch: string;
  authorLogin: string; author: ReviewAuthor; requested: boolean; issue: { identifier: string; url: string } | null; bucket: Bucket; health: PrHealth;
};
export type ReviewFilters = { authors: string[]; statuses: Bucket[] };
export const BUCKET_LABEL: Record<Bucket, string> = { needsReview: "Needs review", draft: "Draft", ciFailing: "CI failing", changesRequested: "Changes requested", approved: "Approved" };
```

- `src/reviewFilter.ts` exports `authorKey(a)`, `authorLabel(a)`, `applyFilters(prs, f)`, `authorsOf(prs)`.
- `ReviewList` props: `{ prs: ReviewPr[]; error: string | null; open: string | null; onOpen: (key: string) => void }`.

- [ ] **Step 1: Write the failing check** (`scripts/reviewFilter.check.ts`; Node 24 runs TS directly)

```ts
import assert from "node:assert/strict";
import { applyFilters, authorsOf, authorKey } from "../src/reviewFilter.ts";

const pr = (n: number, author: any, bucket: string) => ({ key: `o/r#${n}`, author, bucket }) as any;
const amy = { kind: "linear", id: "u1", name: "Amy", avatarUrl: null };
const bo = { kind: "linear", id: "u2", name: "Bo", avatarUrl: null };
const anon = { kind: "unlinked", login: "zed" };
const prs = [pr(1, amy, "draft"), pr(2, bo, "needsReview"), pr(3, anon, "draft"), pr(4, amy, "approved")];
const keys = (v: any[]) => v.map((p) => p.key);

assert.deepEqual(keys(applyFilters(prs, { authors: [], statuses: [] })), ["o/r#1", "o/r#2", "o/r#3", "o/r#4"]); // empty = no filter
assert.deepEqual(keys(applyFilters(prs, { authors: ["u1", "unlinked"], statuses: [] })), ["o/r#1", "o/r#3", "o/r#4"]); // OR within
assert.deepEqual(keys(applyFilters(prs, { authors: ["u1"], statuses: ["draft"] })), ["o/r#1"]); // AND across
assert.deepEqual(keys(applyFilters(prs, { authors: [], statuses: ["draft", "approved"] })), ["o/r#1", "o/r#3", "o/r#4"]);
assert.equal(authorKey(anon as any), "unlinked");
assert.deepEqual(authorsOf(prs).map((a) => a.key), ["u1", "u2", "unlinked"]); // by name, unlinked last, deduped
console.log("reviewFilter ok");
```

- [ ] **Step 2: Run to verify failure**

Run: `node scripts/reviewFilter.check.ts`
Expected: error, cannot find module `../src/reviewFilter.ts`.

- [ ] **Step 3: Implement `src/reviewFilter.ts`** (type-only imports so Node can strip them)

```ts
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
```

Make sure `tsconfig.json`'s `include` does not cover `scripts/` (it should be `["src"]`); if it does, exclude `scripts`.

- [ ] **Step 4: Run the check**

Run: `node scripts/reviewFilter.check.ts`
Expected: `reviewFilter ok`.

- [ ] **Step 5: Implement `ReviewList.tsx` and wire it in**

- Load filters once from `get_settings` (`review_filters`), save with `invoke("set_review_filters", { filters })` on every change.
- Filter bar: two buttons, **Author** and **Status**, each toggling a popover (reuse `.repo-pick .popover` styles by giving it a shared `.popover` class) with checkable rows; author rows show a 16px round avatar (`<img>` when `avatarUrl`, else initials in a circle) + name; status rows show `BUCKET_LABEL`. Active filters render as chips under the bar: `Author: Amy, Bo ×` / `Status: Draft ×` (× clears that dimension). A "Clear" link when any filter is active. Popover closes on outside click (`mousedown` listener on `document` checking `ref.contains`).
- Rows (reuse `.tasks .row` styles): avatar, `repo#number` in `.ident`, title in `.t`, a small "Requested" dot/chip when `requested`, `PrIcon color={SEV[health.overall] ?? "grey"} title={OVERALL[health.overall]}`. Selected row has `on`.
- Empty states: no PRs → "Nothing to review"; filters hide all → "No PRs match these filters".
- `error` → `.banner`.
- In `App.tsx`: `const [rv, setRv] = useState<{ prs: ReviewPr[]; error: string | null }>({ prs: [], error: null })`, load with `invoke("get_reviews")` and `listen("reviews-updated")` in the existing mount effect (add the unlisten to cleanup). Rail Reviews icon shows a count badge (`.count`) of `rv.prs.filter(p => p.requested).length` when > 0.

- [ ] **Step 6: Verify**

Run: `pnpm exec tsc --noEmit && node scripts/reviewFilter.check.ts`. Headless render with the mock returning two PRs (one linear, one unlinked) and check the filter popovers toggle and filter rows.

- [ ] **Step 7: Commit**

```bash
git add src/reviewFilter.ts scripts/reviewFilter.check.ts src/ReviewList.tsx src/types.ts src/App.tsx src/App.css
git commit -m "Reviews list: Linear-user and status filters, remembered"
```

---

### Task 7: Review pane (Overview / Diff / Agent + draft)

**Files:**
- Create: `src/DiffFiles.tsx`, `src/ReviewPane.tsx`
- Modify: `src/ReviewView.tsx` (use `DiffFiles`), `src/App.tsx` (render `ReviewPane`), `src/App.css`

**Interfaces:**
- Consumes: commands `get_pr_diff {key}` → `{ diff, truncated }`, `start_review_agent {key}` → session id, `get_review_draft {key}` → `Draft | null` (rejects with message + raw text), `post_review {key, draft}`, `dismiss_review {key}`, `stop_task {issueId: sessionId}`; `Session` from App's `sessions[pr.sessionId]`; `Terminal` component (`issueId` prop = session id); `Markdown`.
- Produces: `DiffFiles({ diff, truncated, base? })` — the diff renderer extracted from `ReviewView` (the `files()` parser, `cls()`, the facts chips and `<details>` list), used by both panes. `ReviewPane({ pr, session }: { pr: ReviewPr; session?: Session })`. Type `Draft = { summary: string; event: "COMMENT" | "APPROVE" | "REQUEST_CHANGES"; comments: { path: string; line: number; body: string }[] }` in `types.ts`.

- [ ] **Step 1: Extract `DiffFiles`**

Move `File`, `files()`, `cls()` and the JSX from `<div className="facts">` through the `fs.map(...)` block out of `ReviewView.tsx` into `src/DiffFiles.tsx` as `export default function DiffFiles({ diff, truncated, base }: { diff: string; truncated: boolean; base?: string })`. `ReviewView` renders `<DiffFiles diff={r.diff} truncated={r.truncated} base={r.base} />`. Run `pnpm exec tsc --noEmit`; the task-mode Review tab must look identical.

- [ ] **Step 2: Implement `ReviewPane.tsx`**

Structure mirrors `TaskPane` (same `.pane`, `.topbar`, `nav.seg`, `.topbar-right` classes):

- Tabs: `Overview`, `Diff`, `Agent` (Agent shows `AgentIcon status={session?.status}`). Default tab: `agent` if `session?.ptyAlive`, else `overview`.
- Top-right: chip `repo#number ↗` (opens `pr.health.url`), then:
  - session alive → `■ Stop agent` (`stop_task` with `issueId: pr.sessionId`);
  - else → primary `Review with agent` (`start_review_agent`, then switch to Agent tab);
  - `Dismiss` (ghost) when a session exists → `dismiss_review`.
  - Errors go to a `.banner` under the top bar (keep the existing pattern: `setErr(String(e))`).
- **Overview:** `.issue` container: title (h2), author (avatar + `authorLabel`), `Requested` chip, chips for `OVERALL[health.overall]`, CI (`health.ci` + failing checks), unresolved threads, linked Linear issue chip (`issue.identifier ↗` → `openUrl(issue.url)`), then `<Markdown text={pr.body} />` (or "No description").
- **Diff:** load `get_pr_diff` when the tab first opens (and when `pr.health` head changes — key on `pr.health.url + pr.health.title` is fine), show "Loading diff…", then `<DiffFiles .../>`; errors in a banner.
- **Agent:** column layout: `Terminal issueId={pr.sessionId}` (flex 1) when `session?.ptyAlive`, else an empty state with the logo, "The agent hasn't reviewed this PR yet." and the `Review with agent` button. Below it, a **Draft** card (`.card`, max-height 45%, scroll):
  - Poll `get_review_draft` every 4s while the Agent tab is visible, and once whenever `session?.status` changes. Keep a local editable copy; only replace it from disk when the user hasn't edited (`dirty` flag) — otherwise show "The agent updated the draft · Reload" link.
  - `null` → "Waiting for the agent to write .murim/review.json…".
  - Error → banner with the message and raw text in a `<pre>`; post buttons disabled.
  - Editor: summary `<textarea>`; event `<select>` (Comment / Approve / Request changes); comment list: each row `path:line` (mono) + `<textarea>` body + × remove.
  - Buttons: primary `Post review` (label reflects event: "Approve", "Request changes", "Comment"), disabled while busy or no draft. On click `post_review({ key: pr.key, draft })`; on success show "Posted" and clear `dirty`; on error keep the draft and show the GitHub error (a comment on a line outside the diff fails here — the user removes or fixes it and retries).
- `App.tsx`: in reviews mode render `<ReviewPane key={pr.key} pr={pr} session={sessions[pr.sessionId]} />` for the selected `openReview`, else the empty state.
- CSS: `.review-agent { flex: 1; min-height: 0; display: flex; flex-direction: column; }`, `.review-agent .draft { flex: none; max-height: 45%; overflow: auto; margin: 8px 12px; }`, avatar circles `.avatar { width: 16px; height: 16px; border-radius: 50%; }` (share with Task 6 if it already added one).

- [ ] **Step 3: Verify**

Run: `pnpm exec tsc --noEmit && node scripts/reviewFilter.check.ts && (cd src-tauri && cargo test)`. Headless render with the mock: reviews mode, select a PR, check all three tabs render; mock `get_review_draft` returning a draft and check edit + post call payload shape `{ key, draft: { summary, event, comments } }`.

- [ ] **Step 4: Commit**

```bash
git add src/DiffFiles.tsx src/ReviewPane.tsx src/ReviewView.tsx src/App.tsx src/App.css src/types.ts
git commit -m "Review pane: overview, PR diff, agent-drafted review edited and posted on click"
```

---

### Task 8: Docs

**Files:**
- Modify: `README.md` (feature list: **Reviews**; how filters map authors to Linear users; review checkouts live in `~/.murim/reviews/`, nothing is posted until you click), `plan.md` (short section on review mode).

- [ ] **Step 1: Write the docs** — 6–10 lines in README under the feature bullets and Security notes ("the review agent reads untrusted PR code and descriptions; prefer Ask permissions mode").
- [ ] **Step 2: Commit**

```bash
git add README.md plan.md
git commit -m "Docs: review mode"
```
