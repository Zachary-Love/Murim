# Meditations — design

Status: approved in chat 2026-10-13.

## Goal

A third mode for open-ended agent work not tied to an issue ("how does feature X work?", "plan how Y could be
implemented across my stack"). Sessions are plain agent terminals; anything worth keeping is written as a markdown
artifact the user can find and read later.

## Layout

- Rail: **Meditations** icon below Tasks and Reviews. View value `"meditations"` in `localStorage murim.view`.
- Sidebar, two sections:
  - **Sessions:** header with **+ New**. Rows show the name; double-click renames inline (Enter saves, Escape
    cancels, empty name is rejected); ✕ stops and removes the session (artifacts are kept). Agent status icon as in Tasks.
  - **Artifacts:** search input, then `.md` files in the artifacts folder, newest (mtime) first. Row label = first
    `# heading` in the file, else the filename without `.md`.
- Main pane:
  - Session selected: Terminal, with Stop / Resume in a small top bar.
  - Artifact selected: header with the path, **Reveal in Finder** and **Copy path**, then the rendered markdown
    (existing `Markdown` component). Read-only.
- The selected item (session or artifact) is remembered while switching modes, like Tasks/Reviews.

## Sessions

- Id `meditation:<8 hex>`; display `name` is random `adjective-noun` (e.g. `quiet-river`) from short built-in lists.
- `SessionInfo` gains `#[serde(default)] name: String`.
- Runs in `repos_root` (error "Set your repos folder in Settings" when empty), with the Settings agent and
  permission mode, through the existing PTY / hooks / notification / persistence machinery.
- No initial prompt: the user types into the agent.
- Claude gets `--add-dir <artifacts>` and
  `--append-system-prompt "Save documents worth keeping (plans, explanations, notes) as markdown files in <artifacts>, with a descriptive kebab-case filename and a # title."`.
  The same flags are passed again on Resume.
- Custom agents get the env var `MURIM_ARTIFACTS=<artifacts>`.
- Excluded from GitHub PR polling, Tempering and the Tasks list. `title_of` returns the name for notifications.
- Commands: `start_meditation() -> id`, `rename_meditation(id, name)`, `delete_meditation(id)` (kill + forget).
  Resume / stop / pty use the existing commands.

## Artifacts

- Setting `meditations_dir`, default `~/.murim/meditations`, created on first use; editable in Settings.
- `list_artifacts(query) -> [{ path, title, modified }]`: recursive `.md` search; non-empty query matches
  filename or content, case-insensitive; files over 1 MB are skipped for the content match.
- `read_artifact(path) -> String`: canonicalized path must be inside the artifacts folder, else error.
- The list refreshes when the view opens, when any meditation session goes idle (its turn ends), and from ↻.
- Reveal and copy use existing plugins (opener `revealItemInDir`; `navigator.clipboard.writeText`).

## Error handling

- Repos folder unset → banner in the sessions section; **+ New** shows the error.
- Artifacts folder missing → created. A file removed outside Murim disappears on next refresh; if it was open, the
  pane shows "File not found".

## Testing

- Rust unit: random name format; search over a temp folder (name match, content match, non-`.md` ignored, newest
  first); `read_artifact` rejects `../` escapes and symlinks pointing outside; `meditation:` ids skipped by GitHub
  poll and Tempering.
- Mock-backend screenshots: session list, rename, terminal pane, artifact list, search, artifact view.

## Not in scope

Editing artifacts in Murim; linking artifacts to sessions; breaking plans into Linear issues; file watcher.
