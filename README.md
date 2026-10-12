# Murim

One pane of glass for AI feature development. Murim pulls your Linear issues and their GitHub PRs into one
list, puts Claude Code to work on a task with one button, and babysits the PR to merge.

- **Tasks** — your assigned issues in the active Linear cycle, each matched to a local repo.
- **Start Claude** — creates a git worktree on the issue's branch and runs the real `claude` CLI in it.
  It works in the background; you're notified only when Claude has a question.
- **Draft PR review** — after a quality pass, Claude opens a draft PR with its own title and description.
  You review the diff in Murim, then accept (marks it ready for review) or send changes back to Claude.
- **Tempering** — watches CI, review comments and merge conflicts, and invokes Claude only when there's work.
- **Caffeine** — keeps the Mac awake while flows run.

macOS only for now. Version 0.1.0 is early software.

## Install

Download the latest `.dmg` from [Releases](https://github.com/Zachary-Love/Murim/releases/latest), open it, and drag
Murim into Applications. It runs on Apple Silicon and Intel Macs.

Murim isn't signed by Apple yet, so macOS blocks the first launch. Either try to open it, then go to
System Settings → Privacy & Security → **Open Anyway**, or run `xattr -cr /Applications/Murim.app` once.

## Requirements

- [Claude Code](https://docs.claude.com/en/docs/claude-code) (`claude`) logged in with your own account
- [GitHub CLI](https://cli.github.com) (`gh`) logged in (`gh auth login`)
- A Linear personal API key (Linear settings → API → Personal API keys)
- Your repos cloned under one folder (default `~/Documents/GitHub`)
- To build from source: Rust, Node 20+, pnpm

## Build from source

```sh
pnpm install
pnpm tauri dev          # development
pnpm tauri build        # release .app / .dmg
```

Open Settings (gear, bottom left), set the repos folder, paste your Linear key, save.
Issues are matched to repos by a label equal to the repo name; you can always pick the repo by hand.

## How it works

Claude runs as the unmodified interactive CLI in a hidden terminal, with Claude Code hooks reporting its
status to a local server (`127.0.0.1`, random port, per-launch token). Murim's own logic is deterministic:
it polls Linear and GitHub and applies fixed rules, invoking Claude only when there is something to do.
See [plan.md](plan.md) for the design.

## Security notes

- **Secrets:** the Linear key is stored in the macOS keychain. The GitHub token is read from `gh` at runtime.
  Murim stores neither in files.
- **Files Murim writes in your repos:** worktrees live in `~/.murim/worktrees/`. Each gets
  `.claude/settings.local.json` (hooks) and `.murim/`, both added to `.git/info/exclude`, so they are never committed.
- **Claude reads untrusted text.** Issue descriptions, CI logs and PR review comments are passed to Claude.
  Anyone who can comment on your PR can try to steer it. Prefer the **Ask permissions** or
  **Accept edits + dev tools** modes; **Bypass permissions** (`--dangerously-skip-permissions`) lets Claude run
  any command without asking, so use it only on repos and PRs where you trust every contributor.
- **Replies as you:** with Tempering replies on, Claude's replies to review comments are posted under your
  GitHub account. Set replies to Off to disable.

## Releasing

Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, then
push a tag: `git tag v0.2.0 && git push origin v0.2.0`. GitHub Actions builds a universal `.dmg` and publishes
the release.

## License

[MIT](LICENSE)
