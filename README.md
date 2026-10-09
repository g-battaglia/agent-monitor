# agent-monitor — tmux monitor for coding agents

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.89%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Linux-lightgrey.svg)](#)
[![Tests](https://img.shields.io/badge/Tests-80%20passing-green.svg)](#development)

An independent tmux monitor and session explorer for **Claude Code,
Codex, OpenCode, and Pi**. See which agents are running, browse project
folders and conversation history, and resume unfinished work from a
lazygit-style TUI or the CLI. Local, offline, and built around the Unix
philosophy — not a replacement for your terminal workflow.

![agent-monitor tmux session explorer and coding agent monitor TUI](assets/screenshot.png)

```sh
cargo build --release --locked
./target/release/agent-monitor
```

## Overview

`agent-monitor` is an independent tmux companion for **Claude Code,
OpenAI Codex, Pi, and OpenCode**. It indexes local conversation history
into a private SQLite catalog and presents a three-panel terminal UI:
projects, sessions, and conversation preview. No task registration,
mandatory extension, or workspace manager is needed.

| Agent | Local history | Explicit resume command |
|---|---|---|
| Claude Code | `~/.claude/projects` (JSONL) | `claude --resume <id>` |
| OpenAI Codex | `~/.codex/sessions` (rollout JSONL) | `codex resume <id>` |
| OpenCode | `~/.local/share/opencode/opencode.db` (SQLite) | `opencode --session <id>` |
| Pi | `~/.pi/agent/sessions` (JSONL v3) | `pi --session <file>` |

`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, and `XDG_DATA_HOME` override the
corresponding default locations. Pi also supports custom session roots.
Codex names come from its local `session_index.jsonl`; bootstrap instructions
and environment blocks are not used as conversation titles. All four agents
have plugin-free live detection in tmux. Terminals whose
saved session cannot be proven appear as selectable **live terminal**
rows in All and Open, never as invented transcript associations. All keeps open
terminals at the top so they are not buried beneath imported history.

## Why not Herdr?

[Herdr](https://herdr.dev) solves a similar problem, but it is a
server-based workspace manager: a background server, remote manifest
updates, workspace/tab rollups, notifications, and script-wait
primitives. That is a lot of machinery if all you want is to find a
conversation and resume it.

`agent-monitor` follows the Unix philosophy instead:

- **Do one thing.** It monitors and resumes sessions. It does not
  manage your workspaces, send notifications, or run a server.
- **Compose with tmux, don't replace it.** Your panes, sessions, and
  workflow stay yours. Detection is read-only (`list-panes`,
  `capture-pane`): no `send-keys`, no pane closes, no config edits.
  Focusing a pane or creating a resume window requires your action.
- **Independent and offline.** One binary, one SQLite file, zero
  network. No background daemon, no remote manifest fetches, no
  telemetry. It works the same on a laptop and on a headless server.
- **Text in, text out.** Every workflow step is a CLI command with JSON
  output. Pipe it into `jq`, `fzf`, or your own scripts.

The independently implemented screen detector follows
[Herdr's documented live-terminal approach](https://herdr.dev/docs/agents):
scoped rules, prioritized approval/working signals, and explicit fallbacks.
It adds a memory-only working → stable idle transition for **finished**;
no server, remote manifest updates, or mandatory plugin is needed.

## Features

- **Automatic discovery.** Projects, names, and history are derived from
  stored session files. No manual registration and no extension required.
- **Resume-oriented workflow.** Sessions carry an explicit state — history,
  to-resume, or done — plus a short next-step note. States are user
  decisions; activity or idleness never changes them implicitly.
- **Live pane awareness.** Verified extension records (`[O]`), passive
  title/folder hints (`[~]`), and screen-inferred activity
  (`working/finished/idle/waiting/blocked/error/unknown`) distinguish proven openings from
  guesses. Duplicate names are never associated arbitrarily; screen
  state never changes stored work state.
- **Safe resume.** Reopening a closed session uses exact argv
  (see the provider-specific commands above) in the project directory,
  preferring a new tmux window, otherwise a new detached session. Verified or probable
  openings block accidental duplicates.
- **Readable previews.** Paginated newest-first pages, branch selection,
  optional tool output. Reasoning traces and images stay hidden.
- **Search everywhere.** The top bar filters the active panel: projects,
  session names/notes, or the current conversation text.
- **Scriptable.** The full workflow (list, show, resume, annotate, undo,
  sync) is available as CLI commands with JSON output.

## Installation

### Homebrew (macOS and Linux)

Install through the [Homebrew tap](https://github.com/g-battaglia/homebrew-agent-monitor)
on macOS or Linux:

```sh
brew tap g-battaglia/agent-monitor
brew install --HEAD g-battaglia/agent-monitor/agent-monitor
agent-monitor
```

This development formula builds from `main` using Rust and installs tmux
as a dependency. It does not install agents, extensions, or background
services. Tagged, checksummed releases can replace HEAD installation later.

### Build from source

Build and launch the explorer:

```sh
cargo build --release --locked
./target/release/agent-monitor
```

It always opens on **All projects / Open**, regardless of the current
folder or saved filters. `pick --project` explicitly selects a project.
Type `/` to search, `Enter` to open a project or jump to a session, `f` to
switch views (All, To resume, Open, Done), and `?` for the contextual action menu.
The menu shows one category at a time: `Tab` / `Shift-Tab` or `←/→`
changes category; `↑/↓` selects an action and `Enter` runs it. Direct
shortcuts work across categories. Click a category or action with the mouse,
or click outside / press `Esc` to close. Disabled actions remain readable
and explain why they are unavailable (for example, live-only terminals
cannot store notes or decisions). Narrow terminals use a compact category
selector; context text is clipped with an ellipsis rather than overflowing.
Selecting **All projects** clears project/session search but keeps the
current view; choose `f` → All to browse the complete history.

The Projects panel follows the active view and displays a folder tree,
not ambiguous duplicate-name suffixes. Its lower border shows the common
root; nested project folders are indented. Counts show matching/total
sessions. Empty folder metadata is labeled **Unknown folder**, never
shown as a blank project. `i` opens scrollable **Details** for the
highlighted project or session, including the full folder and history storage.
In **Projects** (`1` to focus), `Space` toggles the highlighted branch;
`←/→` fold or navigate it. Click its `▸/▾` triangle to toggle with the mouse.
`C` collapses all branches, `E` expands all; both switch to Tree if needed.
`Space` on **All projects** toggles all branches. Closed branches show how
many project folders are hidden. Ancestor folder groups (names ending `/`)
remain visible in Open even without saved conversations of their own;
`Enter` toggles a group instead of changing project scope. `i` summarizes
all its nested projects. These are navigation folders, never invented sessions.
`v` switches between **Tree** and the original-style **Flat** list; `s`
switches **A–Z / Recent**. `v/s/E/C` work from every panel outside search input
and focus Projects; Flat omits ancestor groups. The panel title shows both choices; all controls
are also in `?`. Recent means latest saved exchange, not inferred activity:
tree siblings use the newest saved update anywhere in their branch, while flat
sorts each individual project. Unknown live-only timestamps never imply recency;
the status line reports how many matching projects have known saved updates. Duplicate flat names include their parent path.
Project search reveals matches inside closed branches without forgetting folds.
Layout, order, and folds last for the current monitor run and never change
the active project, session view, notes, or work decisions.

Every session has an agent badge, such as `[pi][R]` or `[claude][~]`.
`R` refreshes without changing filters. Search for `claude`, `codex`, `pi`,
or `opencode` to find that agent's rows.

Jump directly to one project:

```sh
./target/release/agent-monitor pick --project acme-website
```

## Screenshot

The image above is generated from synthetic data by
`scripts/screenshot.py` (via `cargo run --example layout`), so no real
session content ever appears in the repository. Regenerate it with:

```sh
python3 scripts/screenshot.py
```

## Key bindings

| Key | Action |
|---|---|
| `j/k`, arrows | Move or scroll |
| `h/l`, `Tab` | Switch panel |
| `Space`, `←/→` in Projects | Toggle or navigate a branch |
| `E` / `C` | Expand / collapse all branches (switches to Tree) |
| `v` / `s` | Tree/flat layout / alphabetical/recent order |
| `Enter` | Open the pane, or offer to resume |
| `f` / `p` | Change view / pick project |
| `r` / `d` | Mark to resume / done |
| `n` / `u` | Next-step note / undo |
| `/`, `Ctrl-f`, click the bar | Search projects, names, notes, or text |
| `z` | Expand the conversation |
| `[` / `]` | Older / newer pages |
| `b` / `t` | Branches / tool output |
| `o` / `i` | Browse open panes / project or session Details |
| `gg`, `G`, `Ctrl-d/u` | Jump to ends, half pages |
| `Esc`, `?`, `q` | Back, actions, quit (monitor only) |

![Contextual action menu with readable disabled-action reasons](assets/actions.png)

## CLI reference

```sh
agent-monitor sessions --all --project acme-website
agent-monitor sessions --open --json
agent-monitor agent explain --file screen.txt --agent codex
agent-monitor show <id>
agent-monitor details <id>
agent-monitor details --project acme-website --json
agent-monitor details                       # all-project overview
agent-monitor done <id>
agent-monitor reopen <id>
agent-monitor note <id> 'Check the timeout'
agent-monitor undo <id>
agent-monitor open <id> [--resume]
agent-monitor sync
agent-monitor sources list
agent-monitor sources add /path/to/sessions
agent-monitor integration pi status
```

`<id>` accepts the agent session id or the catalog id shown by
`sessions --json`; ambiguous ids are rejected. `open` jumps to a verified
opening when one exists; otherwise it asks for confirmation (or `--resume`
in scripts) and revalidates identity, folder, and presence before launch.
`sessions --open --json` returns an object with `sessions` and `panes`,
including unidentified agent terminals; other session listings return arrays.
Live-only rows can be focused but cannot carry durable notes or Done decisions.
`details` displays project/session metadata rather than transcript bodies;
`details --project <folder>` includes every agent's sessions in that folder.
In the TUI, `i` and the **Details** menu action show the same information.
Use arrows, `j/k`, Page Up/Down, or the mouse wheel to scroll longer details.

## Live agent activity (no plugin)

Every pane running a recognized agent (Pi, Claude, Codex, OpenCode)
appears in the main All/Open list, the `o` picker, and `sessions --open`,
with a live badge directly in the session row, for example
`[pi][R][O] [working]` or `[claude][~] [finished]`. Verified Pi identity
and inferred terminal activity are independent: both verified and probable
openings are sampled. `i` / Details includes the state and its evidence.
Search for `working`, `finished`, `waiting`, or another state to find matching rows.

| Live state | Meaning |
|---|---|
| `working` | Recognized spinner, interrupt/status line, or title signal |
| `finished` | Observed working, then at least two stable idle snapshots ≥1.5s apart |
| `idle` | Ready/no recognized work; no completed turn was observed |
| `blocked` | Recognized permission/approval UI |
| `waiting` | Recognized interactive selection/question UI |
| `error` | Recognized API/request failure; not a generic tool error |
| `unknown` | No usable signal, empty snapshot, or capture unavailable |
| `mixed` | Multiple openings of one conversation disagree |

The worker samples about every two seconds. Detection uses only the current
terminal viewport (`tmux capture-pane -S 0`), never scrollback, with scoped
bottom nonempty lines and stale-response guards. Captured text is matched
in memory and dropped — never logged or stored. Only bounded fingerprints
and observation counters survive in memory for this monitor run. Process
identity changes, verified Pi session-generation changes, missing panes,
errors, and unavailable captures invalidate completion inference.

**Finished is a guessed end of an observed agent turn, not task success or
Done.** Starting the monitor on an already idle agent reports idle, not
finished. No screen state changes notes, To resume, or durable Done.

`agent explain` shows the provenance behind any classification:

```sh
agent-monitor agent explain %3 --json
agent-monitor agent explain --file screen.txt --agent codex
agent-monitor agent explain %3 --watch --json       # JSON Lines, until Ctrl-C
agent-monitor agent explain %3 --watch --samples 5  # bounded transition watch
```

A one-shot snapshot cannot infer a completed turn; the TUI and `--watch`
can because they observe transitions. Agent diagnostics do not open the state
SQLite database. Local TOML overrides support `priority`,
`bottom_non_empty_lines`, `line_starts_any`, and the legacy ordered predicates;
`finished`/`mixed` are derived states, not snapshot-rule states. Invalid or
oversized overrides fall back to bundled rules with a diagnostic warning.

Sandbox/VM wrappers hide the real agent binary; run them as
`AGENT_MONITOR_AGENT=codex fence -- codex` (per-command only, never
exported globally) to select the right manifest. Custom rules live in
`~/.config/agent-monitor/agent-detection/<agent>.toml` and always win
over the bundled manifests. No rule, manifest, or screen guess ever
changes resume state, notes, or Done decisions.

## Optional presence extension

Pi pane matching works without installation (exact title + folder ⇒
probable `[~]`). For proven identity (`[O]`), including across
in-process session switches, install the bundled Pi extension:

```sh
./target/release/agent-monitor integration pi install
```

Then run `/reload` in each already-open Pi session; new sessions load it
automatically. The monitor never sends input to agent sessions on your
behalf.

## Data and privacy

- Transcripts are read-only; provider JSONL files and the OpenCode database
  remain the source of truth. OpenCode connections use SQLite read-only mode
  and query only session/message/part tables, never account or credential tables.
- State lives in `~/.local/state/agent-monitor/sessions.db` (directory
  0700, database 0600). Override with `AGENT_MONITOR_HOME` or `--data-dir`.
- No network access, no telemetry, no credentials read.
- Display strings are sanitized (ANSI/control/bidi sequences stripped).

## Limits

- Pi JSONL v3, Claude top-level JSONL, Codex rollout JSONL, and OpenCode
  SQLite (`session`/`message`/`part` or `session_v2`/`session_message`).
  Unsupported formats are reported, not migrated. Claude subagent files
  are excluded; older OpenCode JSON-directory storage is not indexed.
- Caps: 512 MiB JSONL files, 8 MiB lines, 100k preview entries. OpenCode
  reads are bounded to 100 sessions per batch and 64 MiB of message data
  per session. Limited previews are labeled.
- Unsaved sessions (`--no-session`, or before the first message) appear
  only while running and cannot be resumed from the catalog.
- Custom session directories are discovered via standard locations and
  `.pi/settings.json`; undeclared paths can be added with `sources add`.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
bun test integrations/pi
python3 scripts/smoke.py
cargo run --release --example latency
python3 scripts/latency.py                 # after the release build
cargo run --example layout                # synthetic preview
```

Tests use temporary directories and stub executables; no real agent
process is ever started. Performance figures are local measurements.

## License

MIT. See [LICENSE](LICENSE).
