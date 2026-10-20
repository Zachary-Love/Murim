//! Pavilion keeps the user's own PRs moving, not just the ones Murim started: recent open PRs of theirs (in a repo
//! cloned under repos_root) that need work get a worktree on the PR branch and an idle session keyed
//! `pr:<owner>/<repo>/<n>` with Tempering on. The PR poll + Tempering then invoke Claude for CI, comments, conflicts.
use crate::github::{self, Ci, PrHealth};
use crate::sessions;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const RECENT_DAYS: i64 = 14;

#[derive(Debug, PartialEq)]
pub struct Want {
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub health: PrHealth,
}

pub fn session_id(owner: &str, repo: &str, n: u64) -> String {
    format!("pr:{owner}/{repo}/{n}")
}

/// Work Tempering can do: failing CI, review comments / requested changes, merge conflicts.
pub fn needs_work(h: &PrHealth) -> bool {
    h.ci == Ci::Failing || h.unresolved_threads > 0 || h.changes_requested || h.conflict
}

/// Search response -> PRs to adopt: opened or really worked on since `since` (RFC 3339; "updated" is bumped by bots
/// and bulk "Update branch" clicks), not a bot's branch, not a fork (we can't push to those).
pub fn wants(j: &Value, since: &str) -> Vec<Want> {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    j["data"]["search"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["isCrossRepository"] == false && (worked_on(n).is_some_and(|d| d >= since) || n["createdAt"].as_str().is_some_and(|d| d >= since)))
        .filter(|n| !["snyk-", "dependabot/", "renovate/"].iter().any(|b| n["headRefName"].as_str().unwrap_or_default().starts_with(b)))
        .filter_map(|n| {
            let health = github::pr_node(n).filter(needs_work)?;
            Some(Want { owner: s(&n["repository"]["owner"]["login"]), repo: s(&n["repository"]["name"]), branch: s(&n["headRefName"]), health })
        })
        .filter(|w| !w.branch.is_empty())
        .collect()
}

/// Newest commit date that's real work: GitHub's "Update branch" merges (committer GitHub) don't count.
fn worked_on(n: &Value) -> Option<&str> {
    n["last"]["nodes"].as_array()?.iter().rev().map(|c| &c["commit"]).find(|c| !(c["committer"]["name"] == "GitHub" && c["messageHeadline"].as_str().is_some_and(|m| m.starts_with("Merge branch "))))?["committedDate"].as_str()
}

fn query() -> String {
    format!("query($q:String!){{search(query:$q,type:ISSUE,first:50){{nodes{{... on PullRequest{{{} headRefName isCrossRepository createdAt last:commits(last:5){{nodes{{commit{{committedDate messageHeadline committer{{name}}}}}}}} repository{{name owner{{login}}}}}}}}}}}}", github::PR_FIELDS)
}

/// Worktree on the PR's own branch at ~/.murim/worktrees/<repo>/pr-<n>, so the agent's pushes land on the PR.
fn worktree(clone: &Path, repo: &str, w: &Want) -> Result<PathBuf, String> {
    let home = PathBuf::from(std::env::var("HOME").map_err(|e| e.to_string())?);
    let wt = home.join(".murim/worktrees").join(repo).join(format!("pr-{}", w.health.number));
    if wt.exists() {
        return Ok(wt);
    }
    std::fs::create_dir_all(wt.parent().unwrap()).map_err(|e| e.to_string())?;
    let b = &w.branch;
    sessions::git(clone, &["fetch", "-q", "origin", b])?;
    let p = wt.to_string_lossy();
    if sessions::git(clone, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).is_ok() {
        // keep the user's local branch (it may hold unpushed work); catch up with the PR when that's a fast-forward
        sessions::git(clone, &["worktree", "add", "-q", &p, b])?;
        let _ = sessions::git(&wt, &["merge", "-q", "--ff-only", &format!("origin/{b}")]);
    } else {
        sessions::git(clone, &["worktree", "add", "-q", "--track", "-b", b, &p, &format!("origin/{b}")])?;
    }
    sessions::write_hooks(&wt)?;
    sessions::exclude_settings(clone)?;
    Ok(wt)
}

/// One pass: search, adopt what's new. Errors are per PR and only logged; the next poll retries.
pub fn run(app: &AppHandle) {
    let Ok(dir) = crate::settings::dir(app) else { return };
    let cfg = crate::settings::load(&dir);
    if !cfg.pavilion.auto_temper || cfg.repos_root.is_empty() || cfg.agent != crate::settings::AgentKind::Claude || crate::pavilion::is_paused(app) {
        return;
    }
    let Ok(tok) = github::token() else { return };
    let since = chrono::Utc::now() - chrono::Duration::days(RECENT_DAYS);
    let q = format!("is:pr is:open author:@me archived:false updated:>={} sort:updated-desc", since.format("%Y-%m-%d"));
    let j = match github::gql(&tok, &query(), json!({ "q": q })) {
        Ok(j) => j,
        Err(e) => return crate::pavilion::olog(format!("adopt: GitHub search failed: {e}")),
    };
    let sess = sessions::get_sessions(app.state());
    // PRs already owned by a Murim session (task branches) or adopted earlier
    let owned: HashSet<String> = github::get_prs(app.state()).prs.into_values().map(|p| p.url).collect();
    let new: Vec<Want> = wants(&j, &since.format("%Y-%m-%dT%H:%M:%SZ").to_string()).into_iter().filter(|w| !owned.contains(&w.health.url) && !sess.contains_key(&session_id(&w.owner, &w.repo, w.health.number))).collect();
    if new.is_empty() {
        return;
    }
    // ponytail: one `git remote` per clone per pass with new PRs; cache if repos_root gets huge
    let clones: HashMap<(String, String), PathBuf> = crate::repo_match::list_repos(Path::new(&cfg.repos_root))
        .into_iter()
        .filter_map(|r| {
            let p = PathBuf::from(r.path);
            let (o, n) = github::parse_remote(&sessions::git(&p, &["remote", "get-url", "origin"]).ok()?)?;
            Some(((o.to_lowercase(), n.to_lowercase()), p))
        })
        .collect();
    let mut adopted = false;
    for w in new {
        let Some(clone) = clones.get(&(w.owner.to_lowercase(), w.repo.to_lowercase())) else {
            crate::pavilion::olog(format!("adopt: {}/{}#{} needs work but isn't cloned under the repos folder", w.owner, w.repo, w.health.number));
            continue;
        };
        let id = session_id(&w.owner, &w.repo, w.health.number);
        match worktree(clone, &w.repo, &w) {
            Ok(wt) => {
                sessions::adopt(app, &id, &wt.to_string_lossy());
                crate::pavilion::olog(format!("adopt: picked up {}#{} (Tempering takes it from here)", w.repo, w.health.number));
                crate::tempering::log(app, &id, format!("Pavilion picked up {}#{}: {}", w.repo, w.health.number, w.health.title));
                adopted = true;
            }
            Err(e) => crate::pavilion::olog(format!("adopt {id}: {e}")),
        }
    }
    if adopted {
        crate::github::poke(app); // poll the new sessions' PRs soon so Tempering acts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(n: u64, extra: Value) -> Value {
        let mut v = json!({"number": n, "url": format!("u{n}"), "title": "t", "state": "OPEN", "isDraft": false, "headRefName": "feat", "isCrossRepository": false,
            "repository": {"name": "api", "owner": {"login": "rz"}}, "last": {"nodes": [{"commit": {"committedDate": "2026-10-18T10:00:00Z"}}]}, "reviewThreads": {"nodes": []}, "commits": {"nodes": []}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn adopts_only_own_branch_prs_that_need_work() {
        let red = json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "FAILURE"}]}}}}]}});
        let j = json!({"data": {"search": {"nodes": [
            node(1, red.clone()),
            node(2, json!({})),                                    // healthy: nothing to do
            node(3, json!({"isCrossRepository": true, "commits": red["commits"]})), // fork: can't push
            node(4, json!({"mergeable": "CONFLICTING"})),
            node(5, json!({"reviewDecision": "CHANGES_REQUESTED"})),
            node(6, json!({"reviewDecision": "CHANGES_REQUESTED", "last": {"nodes": [{"commit": {"committedDate": "2026-09-01T10:00:00Z"}}]}})), // stale
            node(7, json!({"reviewDecision": "CHANGES_REQUESTED", "headRefName": "snyk-upgrade-1"})), // bot branch
            node(8, json!({"reviewDecision": "CHANGES_REQUESTED", "last": {"nodes": [ // old work, recently "Update branch"-ed
                {"commit": {"committedDate": "2026-08-01T10:00:00Z", "messageHeadline": "feat: x", "committer": {"name": "Zach"}}},
                {"commit": {"committedDate": "2026-10-18T10:00:00Z", "messageHeadline": "Merge branch 'main' into x", "committer": {"name": "GitHub"}}}]}})),
            node(9, json!({"reviewDecision": "CHANGES_REQUESTED", "createdAt": "2026-10-10T10:00:00Z", "last": {"nodes": [
                {"commit": {"committedDate": "2026-10-18T10:00:00Z", "messageHeadline": "Merge branch 'main' into x", "committer": {"name": "GitHub"}}}]}})), // opened recently
            {},                                                    // non-PR search hit
        ]}}});
        let w = wants(&j, "2026-10-05T00:00:00Z");
        assert_eq!(w.iter().map(|w| w.health.number).collect::<Vec<_>>(), [1, 4, 5, 9]);
        assert_eq!((w[0].owner.as_str(), w[0].repo.as_str(), w[0].branch.as_str()), ("rz", "api", "feat"));
        assert_eq!(session_id("rz", "api", 1), "pr:rz/api/1");
    }
}
