use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Remembered,
    Exact,
    Semantic,
    Claude,
    Manual,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoMatch {
    pub path: String,
    pub source: Source,
}

// ponytail: 0.8 is a guess; tune against real labels/repos.
const THRESHOLD: f32 = 0.8;

pub fn list_repos(root: &Path) -> Vec<Repo> {
    let mut v: Vec<Repo> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .map(|e| Repo { name: e.file_name().to_string_lossy().into(), path: e.path().to_string_lossy().into() })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| !"-_ .".contains(*c)).flat_map(|c| c.to_lowercase()).collect()
}

pub fn resolve(labels: &[String], issue_title: &str, repos: &[Repo], remembered: &HashMap<String, String>) -> Option<RepoMatch> {
    resolve_with(labels, issue_title, repos, remembered, true, true)
}

pub fn resolve_with(labels: &[String], issue_title: &str, repos: &[Repo], remembered: &HashMap<String, String>, semantic: bool, claude: bool) -> Option<RepoMatch> {
    by_remembered(labels, remembered)
        .or_else(|| by_exact(labels, repos))
        .or_else(|| if semantic { by_semantic(labels, repos) } else { None })
        .or_else(|| if claude { by_claude(labels, issue_title, repos) } else { None })
}

pub fn by_remembered(labels: &[String], remembered: &HashMap<String, String>) -> Option<RepoMatch> {
    labels.iter().find_map(|l| {
        let p = remembered.get(l)?;
        Path::new(p).exists().then(|| RepoMatch { path: p.clone(), source: Source::Remembered })
    })
}

fn by_exact(labels: &[String], repos: &[Repo]) -> Option<RepoMatch> {
    labels.iter().find_map(|l| {
        let n = normalize(l);
        let r = repos.iter().find(|r| normalize(&r.name) == n)?;
        Some(RepoMatch { path: r.path.clone(), source: Source::Exact })
    })
}

type Model = Mutex<fastembed::TextEmbedding>;

fn model() -> Option<&'static Model> {
    static M: OnceLock<Option<Model>> = OnceLock::new();
    M.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let opts = fastembed::InitOptions::new(fastembed::EmbeddingModel::BGESmallENV15)
            .with_cache_dir(format!("{home}/Library/Caches/dev.murim.app/fastembed").into());
        fastembed::TextEmbedding::try_new(opts).map(Mutex::new).map_err(|e| eprintln!("fastembed init failed, skipping semantic tier: {e}")).ok()
    })
    .as_ref()
}

fn embed(texts: Vec<String>) -> Option<Vec<Vec<f32>>> {
    model()?.lock().ok()?.embed(texts, None).map_err(|e| eprintln!("fastembed embed failed: {e}")).ok()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let n = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (n(a) * n(b)).max(f32::EPSILON)
}

fn repo_vecs(repos: &[Repo]) -> Option<Vec<Vec<f32>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<f32>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let missing: Vec<&Repo> = {
        let c = cache.lock().ok()?;
        repos.iter().filter(|r| !c.contains_key(&r.path)).collect()
    };
    if !missing.is_empty() {
        let docs = missing.iter().map(|r| {
            let readme = ["README.md", "readme.md", "README"].iter().find_map(|f| std::fs::read_to_string(Path::new(&r.path).join(f)).ok()).unwrap_or_default();
            format!("{} {}", r.name, readme.chars().take(500).collect::<String>())
        });
        let vecs = embed(docs.collect())?;
        let mut c = cache.lock().ok()?;
        for (r, v) in missing.iter().zip(vecs) {
            c.insert(r.path.clone(), v);
        }
    }
    let c = cache.lock().ok()?;
    repos.iter().map(|r| c.get(&r.path).cloned()).collect()
}

fn by_semantic(labels: &[String], repos: &[Repo]) -> Option<RepoMatch> {
    if labels.is_empty() || repos.is_empty() {
        return None;
    }
    let rv = repo_vecs(repos)?;
    let lv = embed(labels.to_vec())?;
    let (score, i) = lv.iter().flat_map(|l| rv.iter().enumerate().map(move |(i, r)| (cosine(l, r), i))).max_by(|a, b| a.0.total_cmp(&b.0))?;
    (score >= THRESHOLD).then(|| RepoMatch { path: repos[i].path.clone(), source: Source::Semantic })
}

fn by_claude(labels: &[String], title: &str, repos: &[Repo]) -> Option<RepoMatch> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    if repos.is_empty() {
        return None;
    }
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    let prompt = format!(
        "Which repo does this issue belong to? Labels: {}. Title: {title}. Repos: {}. Answer with exactly one repo name from the list, or NONE. No other text.",
        labels.join(", "),
        names.join(", ")
    );
    // Finder-launched apps don't have the shell PATH: use the login shell's claude + env, like sessions do.
    let (vars, path) = crate::sessions::shell_env();
    let mut cmd = Command::new(path.as_deref().unwrap_or("claude"));
    cmd.args(["-p", "--model", "haiku", &prompt]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE") || k.starts_with("AI_AGENT") {
            cmd.env_remove(k);
        }
    }
    cmd.envs(vars.iter().filter(|(k, _)| !(k.starts_with("CLAUDE") || k.starts_with("AI_AGENT"))).map(|(k, v)| (k, v)));
    let mut child = cmd.spawn().map_err(|e| eprintln!("claude spawn failed: {e}")).ok()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match child.try_wait().ok()? {
            Some(s) if s.success() => break,
            Some(_) => return None,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    // ponytail: output is one word, so reading after exit can't fill the pipe.
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout?, &mut out).ok()?;
    let ans = out.trim().trim_matches(|c| c == '`' || c == '"' || c == '.');
    let r = repos.iter().find(|r| r.name == ans)?;
    Some(RepoMatch { path: r.path.clone(), source: Source::Claude })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_tiers() {
        let root = std::env::temp_dir().join(format!("murim-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("My-Repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("alpha/.git")).unwrap();
        std::fs::write(root.join("filegit"), "").unwrap();
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: x").unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();

        let repos = list_repos(&root);
        let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["My-Repo", "alpha", "wt"]);

        let none = HashMap::new();
        let m = resolve_with(&["bug".into(), "my_repo".into()], "t", &repos, &none, false, false).unwrap();
        assert_eq!((m.source, m.path.ends_with("My-Repo")), (Source::Exact, true));
        assert!(resolve_with(&["nope".into()], "t", &repos, &none, false, false).is_none());

        let mut rem = HashMap::new();
        rem.insert("my_repo".to_string(), root.join("alpha").to_string_lossy().to_string());
        let m = resolve_with(&["my_repo".into()], "t", &repos, &rem, false, false).unwrap();
        assert_eq!((m.source, m.path.ends_with("alpha")), (Source::Remembered, true));
        rem.insert("my_repo".to_string(), root.join("gone").to_string_lossy().to_string());
        let m = resolve_with(&["my_repo".into()], "t", &repos, &rem, false, false).unwrap();
        assert_eq!(m.source, Source::Exact); // stale remembered path falls through
        let _ = std::fs::remove_dir_all(&root);
    }
}
