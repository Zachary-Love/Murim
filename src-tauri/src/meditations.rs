//! Meditations: free-form agent sessions in the repos folder, plus the markdown artifacts they leave behind.
use crate::sessions;
use crate::settings;
use serde::Serialize;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const ADJ: [&str; 24] = ["quiet", "still", "calm", "bright", "hidden", "ancient", "gentle", "silver", "misty", "golden", "patient", "wandering", "silent", "distant", "deep", "clear", "soft", "lone", "steady", "hollow", "pale", "swift", "humble", "frozen"];
const NOUN: [&str; 24] = ["river", "mountain", "pine", "lotus", "crane", "stone", "cloud", "willow", "temple", "moon", "valley", "brook", "tiger", "bamboo", "lantern", "peak", "mist", "garden", "path", "bell", "pond", "dragon", "breeze", "ember"];
const MAX_SCAN: u64 = 1 << 20;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub path: String,
    pub title: String,
    pub modified: u64,
}

fn random_name() -> String {
    format!("{}-{}", ADJ[rand::random::<usize>() % ADJ.len()], NOUN[rand::random::<usize>() % NOUN.len()])
}

fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = settings::meditations_dir(&settings::load(&settings::dir(app)?));
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

#[tauri::command]
pub fn start_meditation(app: AppHandle) -> Result<String, String> {
    let root = settings::load(&settings::dir(&app)?).repos_root;
    if root.is_empty() {
        return Err("Set your repos folder in Settings".into());
    }
    let id = format!("meditation:{:08x}", rand::random::<u32>());
    sessions::start_meditation_session(&app, &id, random_name(), &root)?;
    Ok(id)
}

#[tauri::command]
pub fn rename_meditation(app: AppHandle, id: String, name: String) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name can't be empty".into());
    }
    sessions::with_session(&app, &id, |s| s.name = name.into()).ok_or("unknown session".into())
}

#[tauri::command]
pub fn delete_meditation(app: AppHandle, id: String) {
    if sessions::is_meditation(&id) {
        sessions::forget(&app, &id);
    }
}

#[tauri::command]
pub fn list_artifacts(app: AppHandle, query: String) -> Result<Vec<Artifact>, String> {
    Ok(search(&dir(&app)?, &query))
}

#[tauri::command]
pub fn read_artifact(app: AppHandle, path: String) -> Result<String, String> {
    read_in(&dir(&app)?, Path::new(&path))
}

#[tauri::command]
pub fn get_meditations_dir(app: AppHandle) -> Result<String, String> {
    Ok(dir(&app)?.to_string_lossy().into_owned())
}

/// Recursive `.md` scan (hidden entries and symlinks skipped), filename/content match, newest first.
fn search(root: &Path, query: &str) -> Vec<Artifact> {
    let q = query.trim().to_lowercase();
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let (p, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
            let Ok(ft) = e.file_type() else { continue };
            if name.starts_with('.') {
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if !ft.is_file() || p.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let hit = q.is_empty() || name.to_lowercase().contains(&q) || (meta.len() <= MAX_SCAN && std::fs::read_to_string(&p).is_ok_and(|t| t.to_lowercase().contains(&q)));
            if !hit {
                continue;
            }
            let title = std::fs::File::open(&p).ok().and_then(|f| std::io::BufReader::new(f).lines().map_while(Result::ok).find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string())));
            let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            out.push(Artifact { path: p.to_string_lossy().into_owned(), title: title.filter(|t| !t.is_empty()).unwrap_or(stem), modified });
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.modified));
    out
}

/// Trust boundary: the canonical path (symlinks and `..` resolved) must be inside the artifacts folder.
fn read_in(root: &Path, path: &Path) -> Result<String, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let p = path.canonicalize().map_err(|_| "File not found".to_string())?;
    if !p.starts_with(&root) {
        return Err("Not an artifact".into());
    }
    std::fs::read_to_string(p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("murim-med-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str, age: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
        std::fs::File::options().write(true).open(p).unwrap().set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
    }

    #[test]
    fn name_format() {
        for _ in 0..50 {
            let n = random_name();
            let (a, b) = n.split_once('-').unwrap();
            assert!(!a.is_empty() && !b.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '-') && n.matches('-').count() == 1, "{n}");
        }
    }

    #[test]
    fn search_artifacts() {
        let d = tmp("search");
        write(&d.join("auth-flow.md"), "# How Auth Works\nTokens rotate.", 300);
        write(&d.join("sub/plan.md"), "no heading\nUses the Billing API.", 100);
        write(&d.join("notes.txt"), "billing auth", 0);
        write(&d.join(".hidden/x.md"), "billing", 0);
        let titles = |q: &str| search(&d, q).into_iter().map(|a| a.title).collect::<Vec<_>>();
        assert_eq!(titles(""), ["plan", "How Auth Works"]); // newest first, stem fallback, .txt + hidden ignored
        assert_eq!(titles("AUTH-"), ["How Auth Works"]); // name match, case-insensitive
        assert_eq!(titles("billing api"), ["plan"]); // content match
        assert!(titles("nothing-matches").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_stays_inside() {
        let d = tmp("read");
        let arts = d.join("arts");
        write(&arts.join("a.md"), "# A", 0);
        write(&d.join("secret.md"), "secret", 0);
        assert_eq!(read_in(&arts, &arts.join("a.md")).unwrap(), "# A");
        assert!(read_in(&arts, &arts.join("../secret.md")).is_err());
        std::os::unix::fs::symlink(d.join("secret.md"), arts.join("link.md")).unwrap();
        assert!(read_in(&arts, &arts.join("link.md")).is_err());
        assert!(read_in(&arts, &arts.join("gone.md")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn meditations_skip_pr_flow() {
        assert!(sessions::no_pr_flow("meditation:0a1b2c3d") && sessions::no_pr_flow("review:o/r/1"));
        assert!(!sessions::no_pr_flow("issue-abc"));
    }
}
