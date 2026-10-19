//! Docker queue: agent sessions get ~/.murim/bin first on PATH, where a `docker`/`docker-compose` wrapper runs heavy
//! commands (builds, `run`, `compose up/run/build`) in one of N slots so a burst of agents doesn't swamp the Mac.
//! Slots are `lockf` locks on ~/.murim/docker/slot<i>; the OS drops them when a process dies, so nothing gets stuck.
use serde::Serialize;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const SCRIPT: &str = r#"#!/bin/sh
# Murim's docker queue (installed by Murim; agents get ~/.murim/bin first on PATH).
# Heavy commands take one of N slots (~/.murim/docker/slots, 0 = off); everything else goes straight to docker.
q="$HOME/.murim/docker"
name=$(basename "$0")
self=$(cd "$(dirname "$0")" && pwd -P)
real=
oifs=$IFS; IFS=:
for d in $PATH; do
  [ -n "$d" ] && [ -x "$d/$name" ] && [ "$(cd "$d" 2>/dev/null && pwd -P)" != "$self" ] && { real="$d/$name"; break; }
done
IFS=$oifs
[ -n "$real" ] || { echo "murim: $name not found on PATH" >&2; exit 127; }

heavy() {
  a=; b=; skip=
  for x in "$@"; do
    [ -n "$skip" ] && { skip=; continue; }
    case $x in
      -c|--context|-H|--host|--config|-l|--log-level|-f|--file|-p|--project-name|--env-file|--profile|--project-directory|--ansi|--progress|--parallel) skip=1; continue;;
      -*) continue;;
    esac
    if [ -z "$a" ]; then a=$x; else b=$x; break; fi
  done
  [ "$name" = docker-compose ] && { b=$a; a=compose; }
  case "$a:$b" in
    build:*|run:*|buildx:build|builder:build|image:build|container:run|compose:build|compose:up|compose:run) return 0;;
  esac
  return 1
}

n=$(cat "$q/slots" 2>/dev/null); case $n in ''|*[!0-9]*) n=2;; esac
if [ "$n" -eq 0 ] || ! heavy "$@"; then exec "$real" "$@"; fi
mkdir -p "$q/wait"
w="$q/wait/$$"; m="$q/.ran.$$"
trap 'rm -f "$w" "$m"' EXIT
trap 'exit 130' INT TERM
said=
while :; do
  i=1
  while [ "$i" -le "$n" ]; do
    # the marker tells "ran and exited with s" apart from "slot busy"
    W="$w" lockf -k -s -t 0 "$q/slot$i" /bin/sh -c ': > "$0"; rm -f "$W"; exec "$@"' "$m" "$real" "$@"
    s=$?
    [ -e "$m" ] && exit $s
    i=$((i + 1))
  done
  : > "$w"
  [ -n "$said" ] || { echo "murim: all $n docker slots busy, waiting ($(ls "$q/wait" | wc -l | tr -d ' ') queued)" >&2; said=1; }
  sleep 2
done
"#;

fn root() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".murim"))
}

/// Writes the wrappers and the slot count; called at startup. Failures only cost the queue, never the agent.
pub fn install(slots: u32) {
    let Some(r) = root() else { return };
    let bin = r.join("bin");
    let res = fs::create_dir_all(&bin).and_then(|_| {
        for n in ["docker", "docker-compose"] {
            let p = bin.join(n);
            if fs::read_to_string(&p).ok().as_deref() != Some(SCRIPT) {
                fs::write(&p, SCRIPT)?;
            }
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    });
    if let Err(e) = res {
        eprintln!("docker queue: {e}");
    }
    set_slots(slots);
}

/// Settings save: the wrapper reads the file on every call, so this applies to running agents too.
pub fn set_slots(slots: u32) {
    let Some(r) = root() else { return };
    let d = r.join("docker");
    let _ = fs::create_dir_all(&d).and_then(|_| fs::write(d.join("slots"), slots.to_string()));
}

/// Agent PATH with the wrappers first.
pub fn path_with_bin(path: &str) -> String {
    match root() {
        Some(r) => format!("{}:{path}", r.join("bin").display()),
        None => path.into(),
    }
}

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Queue {
    pub running: u32,
    pub waiting: u32,
    pub slots: u32,
}

/// Pavilion header: slots held right now and wrappers waiting (their file is touched every 2s; older = gone).
#[tauri::command]
pub fn docker_queue() -> Queue {
    root().map(|r| queue_in(r.join("docker"))).unwrap_or_default()
}

fn queue_in(d: PathBuf) -> Queue {
    let slots = fs::read_to_string(d.join("slots")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let files = |dir: PathBuf| fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path());
    let running = files(d.clone())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("slot")) && p.file_name().is_some_and(|n| n != "slots"))
        .filter(|p| File::open(p).is_ok_and(|f| f.try_lock().is_err()))
        .count() as u32;
    let fresh = |p: &PathBuf| fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(10));
    let waiting = files(d.join("wait")).filter(fresh).count() as u32;
    Queue { running, waiting, slots }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    // End to end through the real script: a fake docker logs its args and sleeps; one slot serialises two builds.
    #[test]
    fn wrapper_queues_heavy_commands() {
        let t = std::env::temp_dir().join(format!("murim-docker-{}", std::process::id()));
        let (home, fake) = (t.join("home"), t.join("fake"));
        let bin = home.join(".murim/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&fake).unwrap();
        fs::create_dir_all(home.join(".murim/docker")).unwrap();
        fs::write(home.join(".murim/docker/slots"), "1").unwrap();
        fs::write(bin.join("docker"), SCRIPT).unwrap();
        fs::write(fake.join("docker"), "#!/bin/sh\necho \"$*\" >> \"$HOME/log\"\ncase $1 in build) sleep 2;; esac\nexit 3\n").unwrap();
        for p in [bin.join("docker"), fake.join("docker")] {
            fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("{}:{}:/usr/bin:/bin", bin.display(), fake.display());
        let run = |args: &[&str]| Command::new(bin.join("docker")).args(args).env("HOME", &home).env("PATH", &path).stderr(Stdio::null()).spawn().unwrap();

        let start = Instant::now();
        let (a, b) = (run(&["build", "."]), run(&["build", "-t", "x", "."]));
        std::thread::sleep(Duration::from_millis(800));
        let ps = run(&["ps"]).wait_with_output().unwrap();
        assert_eq!(ps.status.code(), Some(3)); // real exit code comes back
        // ps didn't queue: one build still holds the slot and the other still waits
        let q = queue_in(home.join(".murim/docker"));
        assert_eq!((q.running, q.waiting), (1, 1));
        for c in [a, b] {
            assert_eq!(c.wait_with_output().unwrap().status.code(), Some(3));
        }
        assert!(start.elapsed() >= Duration::from_secs(4)); // the builds ran one after the other
        let log = fs::read_to_string(home.join("log")).unwrap();
        assert_eq!(log.lines().filter(|l| l.starts_with("build")).count(), 2); // each ran exactly once
        fs::remove_dir_all(&t).ok();
    }
}
