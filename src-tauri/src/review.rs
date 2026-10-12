//! Pre-PR review: the user reads Claude's diff and proposed PR, then Murim pushes and opens it.
use crate::hooks::PrFlow;
use crate::sessions::{self, git, title_of, with_session, pr_draft};
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
    title: String,
    body: String,
}

fn worktree(app: &AppHandle, id: &str) -> Result<String, String> {
    with_session(app, id, |s| s.worktree.clone()).ok_or_else(|| "no Claude session for this task".into())
}

/// `.murim/pr.md`: title on the first line, body after.
fn parse_pr(md: &str) -> (String, String) {
    let md = md.trim_start();
    let (t, b) = md.split_once('\n').unwrap_or((md, ""));
    (t.trim().trim_start_matches('#').trim().to_string(), b.trim().to_string())
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
    let (title, body) = parse_pr(&std::fs::read_to_string(wt.join(".murim/pr.md")).unwrap_or_default());
    Ok(Review {
        commits: git(wt, &["log", "--oneline", "--no-decorate", &format!("{base}..HEAD")])?.lines().map(String::from).collect(),
        stat: git(wt, &["diff", "--stat", &base])?,
        uncommitted: dirty(wt)?,
        base: base_ref,
        diff,
        truncated,
        title,
        body,
    })
}

#[tauri::command]
pub fn create_pr(app: AppHandle, issue_id: String, title: String, body: String) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("PR title is empty".into());
    }
    let wt = worktree(&app, &issue_id)?;
    if !dirty(Path::new(&wt))?.is_empty() {
        return Err("The worktree has uncommitted changes. Ask Claude to commit them first.".into());
    }
    git(Path::new(&wt), &["push", "-u", "origin", "HEAD"])?;
    // ponytail: still opened as draft; the existing CI rule flips it to ready once checks pass.
    crate::tempering::gh(&wt, &["pr", "create", "--draft", "--title", title.trim(), "--body", &body])?;
    with_session(&app, &issue_id, |s| s.pr_flow = PrFlow::Opened);
    crate::github::poke(&app);
    Ok(())
}

#[tauri::command]
pub fn request_changes(app: AppHandle, issue_id: String, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Describe the changes you want".into());
    }
    let ident = with_session(&app, &issue_id, |s| title_of(s)).ok_or("no Claude session for this task")?; // worktree dir = identifier
    sessions::resume_task(app, issue_id, Some(format!("Review feedback on your changes: {}\n\nApply it and commit (do not push). {}", text.trim(), pr_draft(&ident))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_md() {
        assert_eq!(parse_pr("\n# Add thing\n\nBody\nFixes FOR-1\n"), ("Add thing".into(), "Body\nFixes FOR-1".into()));
        assert_eq!(parse_pr("Only title"), ("Only title".into(), "".into()));
        assert_eq!(parse_pr(""), ("".into(), "".into()));
    }

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
        std::fs::create_dir_all(wt.join(".murim")).unwrap();
        std::fs::write(wt.join(".murim/pr.md"), "Change a\n\nFixes X-1").unwrap();
        std::fs::write(wt.join("a.txt"), "three\n").unwrap(); // uncommitted
        let r = review_at(&wt).unwrap();
        assert_eq!(r.base, "origin/main");
        assert_eq!(r.commits.len(), 1);
        assert!(r.diff.contains("+three") && r.diff.contains("-one"), "{}", r.diff);
        assert!(r.uncommitted.iter().any(|l| l.ends_with("a.txt")));
        assert_eq!((r.title.as_str(), r.body.as_str()), ("Change a", "Fixes X-1"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
