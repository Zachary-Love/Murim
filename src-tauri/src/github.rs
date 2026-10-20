use crate::{sessions, tempering};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Ci {
    Passing,
    Failing,
    Pending,
    None,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Overall {
    Merged,
    Closed,
    Conflict,
    CiFailing,
    ChangesRequested,
    UnresolvedComments,
    Draft,
    CiPending,
    AwaitingReview,
    ReadyToMerge,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrHealth {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
    pub ci: Ci,
    pub failing_checks: Vec<String>,
    pub unresolved_threads: usize,
    pub changes_requested: bool,
    pub conflict: bool,
    pub review_decision: Option<String>,
    pub overall: Overall,
}

#[derive(Serialize, Clone, Default, PartialEq, Debug)]
pub struct Payload {
    pub prs: HashMap<String, PrHealth>,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    payload: Payload,
    /// issues whose merge cleanup already ran this app run (never kill a resumed session twice)
    cleaned: HashSet<String>,
    errs: HashMap<String, String>,
}

#[derive(Default)]
pub struct Github {
    inner: Mutex<Inner>,
    wake: (Mutex<bool>, Condvar),
}

// ---- pure ----

pub fn derive_overall(p: &PrHealth) -> Overall {
    use Overall::*;
    if p.state == PrState::Merged {
        Merged
    } else if p.state == PrState::Closed {
        Closed
    } else if p.conflict {
        Conflict
    } else if p.ci == Ci::Failing {
        CiFailing
    } else if p.changes_requested {
        ChangesRequested
    } else if p.unresolved_threads > 0 {
        UnresolvedComments
    } else if p.is_draft {
        Draft
    } else if p.ci == Ci::Pending {
        CiPending
    } else if !matches!(p.review_decision.as_deref(), None | Some("APPROVED")) {
        AwaitingReview // ponytail: no review rule on the repo (null decision) counts as ready
    } else {
        ReadyToMerge
    }
}

/// GraphQL response -> health of the newest PR for the branch (None = no PR).
pub fn parse_pr(json: &Value) -> Option<PrHealth> {
    json["data"]["repository"]["pullRequests"]["nodes"].get(0).filter(|n| n.is_object()).and_then(pr_node)
}

/// One PullRequest node (PR_FIELDS selection) -> health.
pub fn pr_node(n: &Value) -> Option<PrHealth> {
    let nodes = |v: &Value| v["nodes"].as_array().cloned().unwrap_or_default();
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let rollup = &n["commits"]["nodes"][0]["commit"]["statusCheckRollup"];
    let (mut failing, mut pending) = (vec![], false);
    for c in nodes(&rollup["contexts"]) {
        match c["__typename"].as_str() {
            Some("CheckRun") => match (c["status"].as_str(), c["conclusion"].as_str()) {
                (Some("COMPLETED"), Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE")) => failing.push(s(&c["name"])),
                (Some("COMPLETED"), _) => {}
                _ => pending = true,
            },
            Some("StatusContext") => match c["state"].as_str() {
                Some("FAILURE" | "ERROR") => failing.push(s(&c["context"])),
                Some("PENDING" | "EXPECTED") => pending = true,
                _ => {}
            },
            _ => {}
        }
    }
    let ci = if !rollup.is_object() {
        Ci::None
    } else if !failing.is_empty() {
        Ci::Failing
    } else if pending {
        Ci::Pending
    } else {
        Ci::Passing
    };
    let rd = n["reviewDecision"].as_str().map(String::from);
    let mut p = PrHealth {
        number: n["number"].as_u64()?,
        url: s(&n["url"]),
        title: s(&n["title"]),
        state: match n["state"].as_str()? {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ => PrState::Open,
        },
        is_draft: n["isDraft"].as_bool().unwrap_or(false),
        ci,
        failing_checks: failing,
        unresolved_threads: nodes(&n["reviewThreads"]).iter().filter(|t| t["isResolved"] == false && t["isOutdated"] != true).count(),
        changes_requested: rd.as_deref() == Some("CHANGES_REQUESTED"),
        conflict: n["mergeable"] == "CONFLICTING",
        review_decision: rd,
        overall: Overall::ReadyToMerge,
    };
    p.overall = derive_overall(&p);
    Some(p)
}

/// "owner/name" from an origin URL (https, scp-style ssh, ssh://).
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let u = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let mut it = u.rsplit(['/', ':']);
    let (name, owner) = (it.next()?, it.next()?);
    (!name.is_empty() && !owner.is_empty() && !owner.contains('@')).then(|| (owner.into(), name.into()))
}

// ---- io ----

pub fn token() -> Result<String, String> {
    let out = Command::new("gh").arg("auth").arg("token").envs(sessions::shell_env().0.iter().map(|(k, v)| (k, v))).stdin(std::process::Stdio::null()).output().map_err(|_| "GitHub CLI (gh) not found — install it and run `gh auth login`".to_string())?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !t.is_empty() {
        Ok(t)
    } else {
        Err("Not logged in to GitHub — run `gh auth login`".into())
    }
}

/// GraphQL selection for one PR: shared by the per-branch poll and review mode.
pub const PR_FIELDS: &str = "number url title state isDraft mergeable reviewDecision headRefOid baseRefName reviewThreads(first:100){nodes{id path line isResolved isOutdated comments(last:10){nodes{databaseId author{login} body}}}} latestReviews(first:20){nodes{id state body author{login}}} commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:100){nodes{__typename ... on CheckRun{name conclusion status databaseId checkSuite{workflowRun{databaseId}}} ... on StatusContext{context state}}}}}}}";

fn branch_query() -> String {
    format!("query($o:String!,$n:String!,$b:String!){{viewer{{login}} repository(owner:$o,name:$n){{pullRequests(headRefName:$b,first:1,orderBy:{{field:CREATED_AT,direction:DESC}},states:[OPEN,MERGED,CLOSED]){{nodes{{{PR_FIELDS}}}}}}}}}")
}

pub fn gql(tok: &str, query: &str, vars: Value) -> Result<Value, String> {
    let j: Value = ureq::post("https://api.github.com/graphql")
        .set("Authorization", &format!("bearer {tok}"))
        .set("User-Agent", "murim")
        .timeout(Duration::from_secs(20))
        .send_json(json!({"query": query, "variables": vars}))
        .map_err(|e| format!("GitHub: {e}"))?
        .into_json()
        .map_err(|e| e.to_string())?;
    match j["errors"][0]["message"].as_str() {
        Some(m) => Err(format!("GitHub: {m}")),
        None => Ok(j),
    }
}

fn query(tok: &str, owner: &str, name: &str, branch: &str) -> Result<Value, String> {
    gql(tok, &branch_query(), json!({"o": owner, "n": name, "b": branch}))
}

/// (repo path, owner, name, branch) for a session worktree.
fn locate(app: &AppHandle, wt: &Path, issue_id: &str) -> Result<(std::path::PathBuf, String, String, String), String> {
    let common = sessions::git(wt, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let repo = Path::new(&common).parent().ok_or("bad git dir")?.to_path_buf();
    let (owner, name) = parse_remote(&sessions::git(&repo, &["remote", "get-url", "origin"])?).ok_or("origin is not a GitHub remote")?;
    let branch = sessions::git(wt, &["branch", "--show-current"]).ok().filter(|b| !b.is_empty()).or_else(|| crate::inbox::find(app, issue_id).map(|i| i.issue.branch_name).filter(|b| !b.is_empty())).ok_or("no branch")?;
    Ok((repo, owner, name, branch))
}

fn poll(app: &AppHandle) {
    let gh = app.state::<Github>();
    let sess = sessions::get_sessions(app.state());
    let publish = |f: &dyn Fn(&mut Inner)| {
        let mut g = gh.inner.lock().unwrap();
        let before = g.payload.clone();
        f(&mut g);
        let mut errs: Vec<_> = g.errs.values().cloned().collect();
        errs.sort();
        if g.payload.error.is_none() && !errs.is_empty() {
            g.payload.error = Some(errs.join("; "));
        }
        if g.payload != before {
            let _ = app.emit("prs-updated", &g.payload);
        }
    };
    let tok = match token() {
        Ok(t) => t,
        Err(e) => return publish(&|g| g.payload = Payload { prs: HashMap::new(), error: Some(e.clone()) }),
    };
    let old = gh.inner.lock().unwrap().payload.prs.clone();
    let (mut prs, mut error, mut details) = (HashMap::new(), None, vec![]);
    for (id, s) in &sess {
        if sessions::no_pr_flow(id) {
            continue; // review-mode checkouts are polled by reviews.rs; meditations have no PR
        }
        let wt = Path::new(&s.worktree);
        if !wt.exists() {
            if let Some(p) = old.get(id) {
                prs.insert(id.clone(), p.clone()); // cleaned up after merge: keep showing it
            }
            continue;
        }
        let res = locate(app, wt, id).and_then(|(repo, o, n, b)| query(&tok, &o, &n, &b).map(|j| (repo, b, j)));
        let (repo, branch, j) = match res {
            Ok(r) => r,
            Err(e) => {
                error = Some(format!("{}: {e}", wt.file_name().unwrap_or_default().to_string_lossy()));
                continue;
            }
        };
        let Some(p) = parse_pr(&j) else { continue };
        if p.state == PrState::Merged && gh.inner.lock().unwrap().cleaned.insert(id.clone()) {
            sessions::stop_task(app.clone(), id.clone());
            let r = sessions::git(&repo, &["worktree", "remove", &s.worktree]).and_then(|_| sessions::git(&repo, &["branch", "-D", &branch]));
            let label = wt.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let mut g = gh.inner.lock().unwrap();
            match r {
                Ok(_) => g.errs.remove(id),
                Err(e) => g.errs.insert(id.clone(), format!("{label}: cleanup failed: {e}")),
            };
        }
        tempering::auto_enable(app, id, &p);
        if let Some(d) = tempering::parse_detail(&j, &p) {
            details.push((id.clone(), d, p.clone()));
        }
        prs.insert(id.clone(), p);
    }
    publish(&|g| g.payload = Payload { prs: prs.clone(), error: error.clone() });
    for (id, d, p) in &details {
        tempering::run(app, id, d, p);
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        poll(&app);
        crate::adopt::run(&app); // after poll: it skips PRs Murim's own task sessions already cover
        crate::pavilion::poke(&app);
        let (m, cv) = &app.state::<Github>().wake;
        drop(cv.wait_timeout_while(m.lock().unwrap(), Duration::from_secs(60), |woken| !*woken).unwrap().0);
        *m.lock().unwrap() = false;
    });
}

fn wake(g: &Github) {
    *g.wake.0.lock().unwrap() = true;
    g.wake.1.notify_one();
}

/// After a Stop hook: poll soon (debounced ~5s so a push has landed on GitHub).
pub fn poke(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        wake(&app.state::<Github>());
    });
}

#[tauri::command]
pub fn get_prs(state: State<Github>) -> Payload {
    state.inner.lock().unwrap().payload.clone()
}

#[tauri::command]
pub fn refresh_prs(state: State<Github>) {
    wake(&state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(extra: Value) -> PrHealth {
        let mut n = json!({"number": 7, "url": "u", "title": "t", "state": "OPEN", "isDraft": false, "mergeable": "MERGEABLE", "reviewDecision": "APPROVED",
            "reviewThreads": {"nodes": []}, "latestReviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS", "contexts": {"nodes": [{"__typename": "CheckRun", "name": "build", "conclusion": "SUCCESS", "status": "COMPLETED"}]}}}}]}});
        for (k, v) in extra.as_object().unwrap() {
            n[k] = v.clone();
        }
        parse_pr(&json!({"data": {"repository": {"pullRequests": {"nodes": [n]}}}})).unwrap()
    }

    #[test]
    fn health_fixtures() {
        assert_eq!(parse_pr(&json!({"data": {"repository": {"pullRequests": {"nodes": []}}}})), None);
        let r = pr(json!({}));
        assert_eq!((r.overall, r.ci), (Overall::ReadyToMerge, Ci::Passing));
        assert_eq!(pr(json!({"state": "MERGED", "mergeable": "CONFLICTING"})).overall, Overall::Merged);
        assert_eq!(pr(json!({"state": "CLOSED"})).overall, Overall::Closed);
        assert_eq!(pr(json!({"isDraft": true})).overall, Overall::Draft);
        assert_eq!(pr(json!({"mergeable": "CONFLICTING"})).overall, Overall::Conflict);
        let f = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE", "contexts": {"nodes": [
            {"__typename": "CheckRun", "name": "lint", "conclusion": "TIMED_OUT", "status": "COMPLETED"},
            {"__typename": "CheckRun", "name": "slow", "conclusion": null, "status": "IN_PROGRESS"},
            {"__typename": "StatusContext", "context": "ci/legacy", "state": "ERROR"},
            {"__typename": "CheckRun", "name": "ok", "conclusion": "SUCCESS", "status": "COMPLETED"}]}}}}]}});
        let f = pr(f);
        assert_eq!((f.overall, f.ci, f.failing_checks), (Overall::CiFailing, Ci::Failing, vec!["lint".to_string(), "ci/legacy".into()]));
        let c = pr(json!({"reviewDecision": "CHANGES_REQUESTED"}));
        assert_eq!((c.overall, c.changes_requested), (Overall::ChangesRequested, true));
        let t = json!({"reviewThreads": {"nodes": [{"isResolved": false, "isOutdated": false}, {"isResolved": false, "isOutdated": true}, {"isResolved": true, "isOutdated": false}]}});
        let u = pr(t);
        assert_eq!((u.overall, u.unresolved_threads), (Overall::UnresolvedComments, 1));
        let pend = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "PENDING", "contexts": {"nodes": [{"__typename": "StatusContext", "context": "x", "state": "PENDING"}]}}}}]}});
        assert_eq!(pr(pend).overall, Overall::CiPending);
        assert_eq!(pr(json!({"reviewDecision": "REVIEW_REQUIRED"})).overall, Overall::AwaitingReview);
        let none = pr(json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": null}}]}}));
        assert_eq!(none.ci, Ci::None);
    }

    #[test]
    fn remotes() {
        for u in ["https://github.com/acme/app.git", "https://github.com/acme/app", "git@github.com:acme/app.git", "ssh://git@github.com/acme/app.git\n"] {
            assert_eq!(parse_remote(u), Some(("acme".into(), "app".into())), "{u}");
        }
        assert_eq!(parse_remote(""), None);
    }

    /// `MURIM_LIVE=owner/name/branch cargo test live_query -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_query() {
        let v = std::env::var("MURIM_LIVE").unwrap();
        let p: Vec<_> = v.splitn(3, '/').collect();
        let j = query(&token().unwrap(), p[0], p[1], p[2]).unwrap();
        let h = parse_pr(&j).unwrap();
        let d = tempering::parse_detail(&j, &h).unwrap();
        println!("{h:?}\n{d:?}");
    }

    #[test]
    fn branch_query_unchanged_and_node_parse() {
        let old = "query($o:String!,$n:String!,$b:String!){viewer{login} repository(owner:$o,name:$n){pullRequests(headRefName:$b,first:1,orderBy:{field:CREATED_AT,direction:DESC},states:[OPEN,MERGED,CLOSED]){nodes{number url title state isDraft mergeable reviewDecision headRefOid baseRefName reviewThreads(first:100){nodes{id path line isResolved isOutdated comments(last:10){nodes{databaseId author{login} body}}}} latestReviews(first:20){nodes{id state body author{login}}} commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:100){nodes{__typename ... on CheckRun{name conclusion status databaseId checkSuite{workflowRun{databaseId}}} ... on StatusContext{context state}}}}}}}}}}}";
        assert_eq!(branch_query(), old);
        let n = json!({"number": 7, "url": "u", "title": "t", "state": "OPEN", "isDraft": true, "reviewThreads": {"nodes": []}, "commits": {"nodes": []}});
        let p = pr_node(&n).unwrap();
        assert_eq!((p.number, p.overall), (7, Overall::Draft));
    }

    #[test]
    fn token_error_path() {
        // gh is absent or logged out in CI/dev: must be a message, never a panic
        if let Err(e) = token() {
            assert!(e.contains("gh"));
        }
    }
}
