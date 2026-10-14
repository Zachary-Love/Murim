# Murim

One pane of glass for AI feature development. Murim pulls your Linear issues and their GitHub PRs into one
list, puts Claude Code to work on a task with one button, and babysits the PR to merge.

- **Tasks** — your assigned issues in the active Linear cycle, each matched to a local repo.
- **Start agent** — creates a git worktree on the issue's branch and runs your coding agent in it, with extra
  terminal tabs if you want them. With Claude Code it works in the background and you're notified only when it has a question.
- **Draft PR review** — after a quality pass, Claude opens a draft PR with its own title and description.
  You review the diff in Murim, then accept (marks it ready for review) or send changes back to Claude.
- **Tempering** — watches CI, review comments and merge conflicts, and invokes Claude only when there's work.
- **Reviews** — other people's open PRs: those requesting your review plus your Linear teams' in-progress issues with a
  linked PR. Filter by author and status like in Linear; the author is the assignee of the linked Linear issue (or
  of the issue named in the branch, e.g. `eng-123-fix`), else the PR shows under "Unlinked". Filters are remembered.
  **Review with agent** checks the PR out in `~/.murim/reviews/<repo>/<n>` (the repo must be cloned under your repos
  folder) and has your agent draft a review into `.murim/review.json`. You edit the summary, verdict and line comments in
  Murim; nothing is posted to GitHub until you click Post. The checkout is removed on Dismiss or when the PR closes.
- **Meditations** — free-form agent sessions in your repos folder for questions and plans not tied to an issue
  ("how does X work?", "plan Y across my stack"). Sessions get a random name you can rename. The agent saves anything
  worth keeping as markdown in the Meditations folder (Settings, default `~/.murim/meditations`); Murim lists and
  searches those artifacts and renders them. Meditations use your Settings permission mode, on your real checkouts.
- **Automations** — scheduled agent runs: a prompt, a folder and a schedule (daily, weekdays, weekly, every 2 weeks
  from a start date, or monthly). Each run is its own session you can open. When a run waits on you (e.g. Claude asks
  before sending an email) you get a "<name> needs you" notification and a dot on the rail. Murim must be open and
  the Mac awake; a run missed within 24h runs once on launch/wake, older ones show as missed.
- **Caffeine** — keeps the Mac awake while flows run.

macOS only for now. Version 0.2.0 is early software.

## Install

Download the latest `.dmg` from [Releases](https://github.com/Zachary-Love/Murim/releases/latest), open it, and drag
Murim into Applications. Requires an Apple Silicon Mac (M1 or newer).

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
Issues are matched to repos by a label equal to the repo name (or a label you mapped before). Otherwise, with Claude
Code, **Start agent** opens Claude in the repos folder with the issue, the repo list and Murim's best guess; it only
reads, names the repo with a final `MURIM_REPO: <name>` line, and Murim moves the task into that repo's worktree.
The folder button in the task bar picks the repo by hand at any time. Custom agents need a repo (or the guess) first.

## Agents

Settings → Agent picks what **Start agent** runs:

- **Claude Code** (default) — full automation: status and question relay via Claude Code hooks, the quality pass and
  draft PR, and Tempering.
- **Custom command** — any CLI agent (`codex`, `gemini`, `aider --yes`, …). It runs in the worktree terminal through
  your login shell with the issue as its last argument. You get the terminal, extra terminals, the Review tab and
  Accept PR; question relay, the automatic quality pass and Tempering need Claude Code's hooks, so they're off.

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
- **Review agents read untrusted code.** A PR's diff and description come from its author and can try to steer the
  agent (for example, into running a script from the PR). The review agent starts in your default permission mode;
  prefer **Ask permissions** for reviews. It is told not to post, and Murim only posts the draft when you click.
- **Replies as you:** with Tempering replies on, Claude's replies to review comments are posted under your
  GitHub account. Set Review replies to Off in Settings to disable.

## Releasing

Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, then
push a tag: `git tag v0.2.0 && git push origin v0.2.0`. GitHub Actions builds the Apple Silicon `.dmg` and publishes
the release.

Releases are signed for the in-app updater, so the repo needs the `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` Actions secrets. The private key lives at `~/.tauri/murim.key` (public key in
`tauri.conf.json`). Back it up: if it's lost, existing installs can never update again. 0.1.0 shipped without the
updater, so 0.1.0 users have to install the next release manually once.

## License

[MIT](LICENSE)
