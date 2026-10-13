# Meditations Implementation Plan

**Goal:** Third rail mode: free-form agent sessions in the repos folder plus a searchable list of markdown artifacts.

**Spec:** docs/specs/2026-10-13-meditations-design.md (read it first; it is the source of truth).

**Architecture:** A meditation is an ordinary session in `sessions.rs` keyed `meditation:<8 hex>`, cwd = repos_root,
hooks via `--settings` (nothing written into repos_root). Artifacts are plain `.md` files in a Settings folder, scanned
on demand by a new `meditations.rs`. Frontend adds a rail icon, a sidebar (sessions + artifacts) and a main pane.

## Global constraints

- Match surrounding style: terse, few comments, snake_case settings on the wire, camelCase structs (`rename_all`).
- No new dependencies (Rust or npm). Random hex/name from `std` (e.g. `SystemTime` nanos + `std::process::id`, or
  `RandomState` hashing) — no `rand` crate.
- Never add Co-Authored-By / "Generated with Claude Code" anywhere. Do NOT commit; the coordinator commits.

## Interfaces (shared contract)

Settings (snake_case): `meditations_dir: String` — empty means default `~/.murim/meditations`.
`settings::meditations_dir(&Settings) -> PathBuf` resolves the default.

SessionInfo gains `#[serde(default)] pub name: String` (camelCase on the wire: `name`).

Tauri commands:
- `start_meditation() -> String` (new session id; spawns the agent with no prompt)
- `rename_meditation(id: String, name: String) -> Result<(), String>` (trimmed; empty → Err("Name can't be empty"))
- `delete_meditation(id: String)` (kill PTY + forget; artifacts untouched)
- `list_artifacts(query: String) -> Result<Vec<Artifact>, String>`, `Artifact { path: String, title: String, modified: u64 /* unix secs */ }`, newest first
- `read_artifact(path: String) -> Result<String, String>` (canonicalized path must be inside the canonicalized dir)
- `get_meditations_dir() -> Result<String, String>` (resolved absolute path, created if missing)
- Existing: `resume_task(issueId, prompt)`, `stop_task(issueId)`, `get_sessions`, `pty_*` work with meditation ids.

## Task A — backend (src-tauri)

Files: `src/meditations.rs` (new), `src/sessions.rs`, `src/settings.rs`, `src/lib.rs`, `src/github.rs`,
`src/tempering.rs`, `capabilities/default.json`.

1. settings.rs: `meditations_dir` field + resolver; extend the round-trip test.
2. sessions.rs:
   - `name` field on SessionInfo (update all struct literals).
   - `pub fn start_meditation_session(app, id, name, repos_root)` modeled on `start_review`; `worktree` = repos_root.
   - In `spawn`, when the id starts with `meditation:` and agent is Claude: add `--settings <merge_hooks(json!({}))>`
     (like the repo-pick branch, but WITHOUT `--disallowedTools`), `--add-dir <dir>`, and
     `--append-system-prompt "Save documents worth keeping (plans, explanations, notes) as markdown files in <dir>, with a descriptive kebab-case filename and a # title."`.
     Must also apply on resume (resume goes through spawn). Custom agent: env `MURIM_ARTIFACTS=<dir>`.
     Make sure task worktree hook-writing (`write_hooks`) never targets repos_root for meditations.
   - `title_of`: meditation → its `name`.
   - `on_hook`: meditations must skip all PR-flow / repo-pick / MURIM_* marker logic — only status changes
     (working / idle / needsInput / ended) and notifications. Check every branch.
3. github.rs poll and tempering.rs consume: skip `meditation:` ids exactly like `review:`.
4. meditations.rs: commands above; random name `adjective-noun` from ~20+20 built-in words; recursive `.md` scan
   (skip hidden dirs), title = first line starting with `# ` else file stem, content match skips files > 1 MB.
5. lib.rs: register commands. capabilities: add `opener:allow-reveal-item-in-dir`.
6. Tests (in meditations.rs, temp dirs under std::env::temp_dir()): name format `^[a-z]+-[a-z]+$`; search name match,
   content match, case-insensitive, non-.md ignored, newest first; read_artifact rejects `../` and an outside symlink;
   a pure helper like `skip_id(id)` (or equivalent) proving poll/tempering skip `meditation:`.
7. `cargo test` and `cargo clippy` clean.

## Task B — frontend (src)

Files: `App.tsx`, `Icons.tsx` (MeditationsIcon, stroke style matching TasksIcon/ReviewsIcon), `types.ts`
(`Artifact`), new `MeditationList.tsx`, new `MeditationPane.tsx` (terminal view + artifact view), `Settings.tsx`
(Meditations folder text input + Browse via dialog `open({directory:true})`, like repos folder), `App.css`.

1. View `"meditations"` in the rail (below Reviews) and in `murim.view`; `openMeditation` selection state
   `{kind:"session"|"artifact", id}` kept per mode.
2. Sidebar: "Meditations" section label with count + **+ New** (calls start_meditation, selects it, shows errors in a
   banner); session rows from `sessions` entries whose key starts with `meditation:` (sorted by name), AgentIcon
   status, double-click inline rename (Enter save / Escape cancel), ✕ delete. "Artifacts" section: search input
   (debounced ~200ms) → list_artifacts; rows show title + relative date.
3. Refresh artifacts on view open, on ↻ (refresh button in brand header calls it in this view), and whenever a
   meditation session's status transitions to idle/ended (compare previous sessions in a ref).
4. Pane: session → small top bar (name, Stop / Resume like TaskPane) + `<Terminal key={id} issueId={id}/>`
   (read Terminal props in TaskPane for exact usage). Artifact → header (path in mono, Reveal via
   `revealItemInDir` from `@tauri-apps/plugin-opener`, Copy path via navigator.clipboard with "Copied" flash),
   body `<Markdown text=…/>`; read error → "File not found".
5. Tasks/Reviews lists must not show meditation sessions (verify).
6. Mock check: scratchpad preview2/mock.js — add handlers for the new commands with 2 sessions + 3 artifacts, run
   `pv.sh`, then `shot.sh <name> "<hash>"` (read mock.js to see how the hash drives state). Screenshot: list, rename
   input, terminal pane, artifact view, search. `pnpm exec tsc --noEmit` clean.
