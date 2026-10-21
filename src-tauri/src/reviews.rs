//! Review mode: other people's PRs (GitHub review requests + the team's Linear-linked PRs), authors as Linear users,
//! and an agent that drafts a review in a detached checkout. Nothing is posted until the user clicks.
use crate::github::{pr_node, Overall, PrHealth};
use crate::sessions::{self, git};
use crate::{github, linear};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    /// a person (not a bot like Bugbot) already reviewed it
    pub reviewed: bool,
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
        reviewed: n["latestReviews"]["nodes"].as_array().is_some_and(|a| a.iter().any(|r| r["author"]["__typename"] != "Bot")),
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
    /// false until the first poll finishes (the frontend shows a loader instead of "Nothing to review")
    pub loaded: bool,
}

#[derive(Default)]
pub struct Reviews {
    payload: Mutex<Payload>,
    /// identifier -> Linear issue (None = no such issue); per app run
    idents: Mutex<HashMap<String, Option<(LinkedIssue, Option<Assignee>)>>>,
    wake: (Mutex<bool>, Condvar),
}

fn search_query() -> String {
    format!("query($q:String!){{viewer{{login}} search(query:$q,type:ISSUE,first:50){{nodes{{... on PullRequest{{{} body headRefName author{{login}} latestReviews(first:10){{nodes{{author{{__typename}}}}}} repository{{name owner{{login}}}}}}}}}}}}", github::PR_HEALTH)
}

/// Many PR URLs in one query (aliases p0, p1, …): one point for the batch instead of one per PR.
pub fn urls_query(n: usize) -> String {
    let vars: Vec<String> = (0..n).map(|i| format!("$u{i}:URI!")).collect();
    let body: String = (0..n).map(|i| format!("p{i}:resource(url:$u{i}){{... on PullRequest{{{} body headRefName author{{login}} latestReviews(first:10){{nodes{{author{{__typename}}}}}} repository{{name owner{{login}}}}}}}} ", github::PR_HEALTH)).collect();
    format!("query({}){{{body}}}", vars.join(","))
}

/// (viewer login, PRs where GitHub asks the viewer or one of their teams to review).
fn requested(tok: &str) -> Result<(String, Vec<ReviewPr>), String> {
    let j = github::gql(tok, &search_query(), json!({"q": "is:pr is:open review-requested:@me archived:false"}))?;
    let me = j["data"]["viewer"]["login"].as_str().unwrap_or_default().to_string();
    Ok((me, j["data"]["search"]["nodes"].as_array().into_iter().flatten().filter_map(|n| to_review(n, true)).collect()))
}

/// The team's Linear-linked PRs, minus URLs already fetched via search; 50 per GitHub query.
fn team(tok: &str, skip: &[String]) -> Result<Vec<ReviewPr>, String> {
    let key = linear::api_key()?;
    let mut links = linear::team_pr_links(&key)?;
    links.retain(|(u, ..)| !skip.contains(u));
    let mut seen = std::collections::HashSet::new();
    links.retain(|(u, ..)| seen.insert(u.clone()));
    let mut out: Vec<ReviewPr> = vec![];
    for chunk in links.chunks(50) {
        let vars: serde_json::Map<String, Value> = chunk.iter().enumerate().map(|(i, (u, ..))| (format!("u{i}"), json!(u))).collect();
        let j = github::gql(tok, &urls_query(chunk.len()), Value::Object(vars))?;
        for (i, (_, issue, who)) in chunk.iter().enumerate() {
            if let Some(mut p) = to_review(&j["data"][format!("p{i}")], false) {
                link(&mut p, issue.clone(), who.clone());
                out.push(p);
            }
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
    Payload { prs, error: (!errs.is_empty()).then(|| errs.join("; ")), loaded: true }
}

fn poll(app: &AppHandle) {
    let p = match github::token() {
        Err(e) => Payload { prs: vec![], error: Some(e), loaded: true },
        Ok(tok) => {
            let gh = requested(&tok);
            let skip: Vec<String> = gh.as_ref().map(|(_, v)| v.iter().map(|p| p.health.url.clone()).collect()).unwrap_or_default();
            let t = team(&tok, &skip);
            let mut p = combine(gh, t);
            link_by_branch(app, &mut p.prs);
            p
        }
    };
    // only a successful fetch can say a PR closed; offline must not wipe review checkouts
    if p.error.is_none() {
        let root = crate::settings::dir(app).map(|d| crate::settings::load(&d).repos_root).unwrap_or_default();
        let ids: Vec<String> = sessions::get_sessions(app.state()).into_keys().collect();
        for id in stale(&ids, &p.prs) {
            // dropping out of the list isn't enough (a Linear-only PR leaves when its issue changes state): only a closed PR loses its checkout
            let Some((o, r, n)) = parse_session(&id) else { continue };
            if !closed(&o, &r, n) {
                continue;
            }
            sessions::forget(app, &id);
            if let Ok(h) = home() {
                if let Some(c) = find_clone(Path::new(&root), &o, &r) {
                    let _ = remove_review_worktree(&c, &h.join(".murim/reviews").join(&r).join(n.to_string()), n);
                }
            }
        }
    }
    let st = app.state::<Reviews>();
    let mut cur = st.payload.lock().unwrap();
    let mut p = p;
    if p.error.is_some() {
        // a failed source (e.g. rate limit) keeps its last rows instead of emptying the list
        let old = std::mem::take(&mut cur.prs);
        p.prs = merge(p.prs, old);
    }
    *cur = p;
    let _ = app.emit("reviews-updated", &*cur); // also when unchanged: it ends the frontend's refresh loader
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        poll(&app);
        crate::pavilion::poke(&app);
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
/// True only when GitHub confirms the PR is merged or closed; any error counts as open.
fn closed(owner: &str, repo: &str, n: u64) -> bool {
    github::token()
        .and_then(|t| github::gql(&t, "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){state}}}", json!({"o": owner, "r": repo, "n": n})))
        .is_ok_and(|j| matches!(j["data"]["repository"]["pullRequest"]["state"].as_str(), Some("MERGED" | "CLOSED")))
}

pub fn stale(session_ids: &[String], open: &[ReviewPr]) -> Vec<String> {
    session_ids.iter().filter(|id| id.starts_with("review:") && !open.iter().any(|p| &p.session_id == *id)).cloned().collect()
}

/// `extra`: Settings → Review prompt, appended under "Instructions:" like sessions.rs does default_prompt.
fn review_prompt(p: &ReviewPr, extra: &str) -> String {
    let extra = extra.trim();
    let extra = if extra.is_empty() { String::new() } else { format!("\n\nInstructions:\n{extra}") };
    let who = match &p.author { Author::Linear { name, .. } => name.clone(), Author::Unlinked { login } => login.clone() };
    format!("You are reviewing pull request {url} \"{title}\" by {who}. Its code is checked out here (detached HEAD); the base branch is origin/{base}. Read the change with `git diff origin/{base}...HEAD` and the surrounding code.\n\nDo NOT post to GitHub, comment, commit or push. Murim posts the review after the user approves it.\n\nWrite your review to .murim/review.json as JSON: {{\"summary\": string (markdown), \"event\": \"COMMENT\" | \"APPROVE\" | \"REQUEST_CHANGES\", \"comments\": [{{\"path\": string, \"line\": number (line in the new version of the file; must be inside the diff), \"body\": string}}]}}. Write the file in one go, then say it is ready.\n\nPR description:\n{body}{extra}",
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

/// The review agent wrote its draft (Pavilion: "review draft ready").
pub fn has_draft(p: &ReviewPr) -> bool {
    wt_of(p).is_ok_and(|w| w.join(".murim/review.json").exists())
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
    let cfg = crate::settings::load(&crate::settings::dir(&app)?);
    sessions::start_review(&app, &p.session_id, &wt.to_string_lossy(), review_prompt(&p, &cfg.review_prompt))?;
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
    // posted: the draft is no longer waiting on the user, so Pavilion's "review draft ready" (has_draft) must clear
    if let Ok(w) = wt_of(&p) {
        let _ = std::fs::rename(w.join(".murim/review.json"), w.join(".murim/review.posted.json"));
    }
    sessions::with_session(&app, &p.session_id, |s| {
        if s.status == crate::hooks::Status::NeedsInput {
            s.status = crate::hooks::Status::Idle; // nothing left to answer once the review is out
        }
    });
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

/// "review:<owner>/<repo>/<n>" -> parts.
fn parse_session(id: &str) -> Option<(String, String, u64)> {
    let mut it = id.strip_prefix("review:")?.splitn(3, '/');
    Some((it.next()?.into(), it.next()?.into(), it.next()?.parse().ok()?))
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
    fn auto_review_only_when_ci_passes_and_no_person_reviewed() {
        let mut n = node(1, "bob", "x");
        n["commits"] = json!({"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS", "contexts": {"nodes": []}}}}]});
        let p = to_review(&n, true).unwrap();
        assert!(crate::pavilion::wants_review(&p, false));
        assert!(!crate::pavilion::wants_review(&p, true)); // our agent already has a session
        n["latestReviews"] = json!({"nodes": [{"author": {"__typename": "Bot"}}]});
        assert!(crate::pavilion::wants_review(&to_review(&n, true).unwrap(), false)); // Bugbot doesn't count
        n["latestReviews"] = json!({"nodes": [{"author": {"__typename": "Bot"}}, {"author": {"__typename": "User"}}]});
        assert!(!crate::pavilion::wants_review(&to_review(&n, true).unwrap(), false));
        assert!(!crate::pavilion::wants_review(&to_review(&node(2, "bob", "x"), true).unwrap(), false)); // no checks
    }

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
        assert_eq!(parse_session("review:o/r/5"), Some(("o".into(), "r".into(), 5)));
        assert_eq!(parse_session("issue-1"), None);
    }

    #[test]
    fn review_prompt_appends_setting() {
        let p = to_review(&node(1, "amy", "main"), true).unwrap();
        assert!(review_prompt(&p, "  ").ends_with("PR description:\nb"));
        assert!(review_prompt(&p, " check tests \n").ends_with("PR description:\nb\n\nInstructions:\ncheck tests"));
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
}
