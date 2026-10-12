//! Draft-PR review: Claude opens a draft PR; the user reads the diff + description, then accepts (marks it ready) or asks for changes.
use crate::hooks::PrFlow;
use crate::sessions::{self, changes_prompt, git, with_session};
use serde::Serialize;
use std::path::Path;
use tauri::AppHandle;

/// ponytail: cap what we ship to the webview; huge diffs (lockfiles, generated code) are truncated, not paged.
const MAX_DIFF: usize = 1_500_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    base: String,
    commits: Vec<String>,
    stat: String,
    diff: String,
    truncated: bool,
    uncommitted: Vec<String>,
    /// local commits not on the remote branch yet
    unpushed: usize,
    /// the draft PR as on GitHub (`gh pr view` JSON), null before Claude opens it
    pr: Option<serde_json::Value>,
}

fn worktree(app: &AppHandle, id: &str) -> Result<String, String> {
    with_session(app, id, |s| s.worktree.clone()).filter(|w| !w.is_empty()).ok_or_else(|| "no Claude session for this task".into())
}

fn dirty(wt: &Path) -> Result<Vec<String>, String> {
    Ok(git(wt, &["status", "--porcelain"])?.lines().map(String::from).collect())
}

#[tauri::command]
pub fn get_review(app: AppHandle, issue_id: String) -> Result<Review, String> {
    review_at(Path::new(&worktree(&app, &issue_id)?))
}

fn review_at(wt: &Path) -> Result<Review, String> {
    let base_ref = git(wt, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).unwrap_or_else(|_| "origin/main".into());
    let base = git(wt, &["merge-base", &base_ref, "HEAD"])?;
    // diff against the merge base includes uncommitted edits, so the user sees exactly what Claude left
    let mut diff = git(wt, &["diff", "--no-color", &base])?;
    let truncated = diff.len() > MAX_DIFF;
    if truncated {
        let mut cut = MAX_DIFF;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
    }
    Ok(Review {
        unpushed: unpushed(wt),
        pr: crate::tempering::gh(&wt.to_string_lossy(), &["pr", "view", "--json", "number,url,title,body,isDraft,state"]).ok().and_then(|o| serde_json::from_str(&o).ok()),
        commits: git(wt, &["log", "--oneline", "--no-decorate", &format!("{base}..HEAD")])?.lines().map(String::from).collect(),
        stat: git(wt, &["diff", "--stat", &base])?,
        uncommitted: dirty(wt)?,
        base: base_ref,
        diff,
        truncated,
    })
}

/// No upstream yet counts as everything unpushed.
fn unpushed(wt: &Path) -> usize {
    git(wt, &["rev-list", "--count", "@{u}..HEAD"]).ok().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX)
}

/// The user accepts Claude's draft: mark it ready for review.
#[tauri::command]
pub fn accept_pr(app: AppHandle, issue_id: String) -> Result<(), String> {
    let wt = worktree(&app, &issue_id)?;
    let p = Path::new(&wt);
    if !dirty(p)?.is_empty() || unpushed(p) > 0 {
        return Err("The branch has changes that aren't on the PR yet. Ask Claude to commit and push them first.".into());
    }
    crate::tempering::gh(&wt, &["pr", "ready"])?;
    with_session(&app, &issue_id, |s| s.pr_flow = PrFlow::Ready);
    crate::github::poke(&app);
    Ok(())
}

#[tauri::command]
pub fn request_changes(app: AppHandle, issue_id: String, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Describe the changes you want".into());
    }
    sessions::resume_task(app, issue_id, Some(changes_prompt(text.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_of_branch_with_uncommitted_edit() {
        let root = std::env::temp_dir().join(format!("murim-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (origin, wt) = (root.join("origin.git"), root.join("wt"));
        std::fs::create_dir_all(&root).unwrap();
        let g = |dir: &Path, a: &[&str]| git(dir, a).unwrap();
        g(&root, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        g(&root, &["clone", "-q", origin.to_str().unwrap(), wt.to_str().unwrap()]);
        let c = |m: &str| g(&wt, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", m]);
        std::fs::write(wt.join("a.txt"), "one\n").unwrap();
        g(&wt, &["add", "."]);
        c("base");
        g(&wt, &["push", "-q", "-u", "origin", "main"]);
        g(&wt, &["remote", "set-head", "origin", "main"]);
        g(&wt, &["checkout", "-qb", "feat"]);
        std::fs::write(wt.join("a.txt"), "two\n").unwrap();
        c("change a");
        std::fs::write(wt.join("a.txt"), "three\n").unwrap(); // uncommitted
        let r = review_at(&wt).unwrap();
        assert_eq!(r.base, "origin/main");
        assert_eq!(r.commits.len(), 1);
        assert!(r.diff.contains("+three") && r.diff.contains("-one"), "{}", r.diff);
        assert!(r.uncommitted.iter().any(|l| l.ends_with("a.txt")));
        assert_eq!(r.unpushed, usize::MAX); // feat has no upstream
        g(&wt, &["push", "-q", "-u", "origin", "feat"]);
        assert_eq!(unpushed(&wt), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
