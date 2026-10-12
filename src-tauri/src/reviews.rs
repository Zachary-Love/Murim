//! Review mode: other people's PRs (GitHub review requests + the team's Linear-linked PRs), authors as Linear users,
//! and an agent that drafts a review in a detached checkout. Nothing is posted until the user clicks.
use crate::github::{pr_node, Overall, PrHealth};
use crate::{github, linear};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

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
}
