# agent-monitor — tmux monitor for coding agents

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.89%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Linux-lightgrey.svg)](#)
[![Tests](https://img.shields.io/badge/Tests-57%20passing-green.svg)](#development)

A terminal session explorer for coding agents (Pi, Claude, Codex,
OpenCode). Browse past conversations by project, preview the full
exchange, and resume exactly where work stopped — from the TUI or from
scripts. A lightweight, independent tmux-native alternative to bloated
agent dashboards.

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
| Pi | `~/.pi/agent/sessions` (JSONL v3) | `pi --session <file>` |
| Claude Code | `~/.claude/projects` (JSONL) | `claude --resume <id>` |
| OpenAI Codex | `~/.codex/sessions` (rollout JSONL) | `codex resume <id>` |
| OpenCode | `~/.local/share/opencode/opencode.db` (SQLite) | `opencode --session <id>` |

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

The screen-activity detection is inspired by Herdr's manifests
(`working/idle/blocked/unknown` from terminal output, no plugin), minus
the server and remote manifest updates.

## Features

- **Automatic discovery.** Projects, names, and history are derived from
  stored session files. No manual registration and no extension required.
- **Resume-oriented workflow.** Sessions carry an explicit state — history,
  to-resume, or done — plus a short next-step note. States are user
  decisions; activity or idleness never changes them implicitly.
- **Live pane awareness.** Verified extension records (`[O]`), passive
  title/folder hints (`[~]`), and screen-inferred activity
  (`working/idle/blocked/unknown`) distinguish proven openings from
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

It opens on **All** sessions. Type `/` to search, `Enter` to open a
project or jump to a session, `f` to switch views (All, To resume, Open,
Done), `?` for the contextual action menu. Selecting **All projects**
with `Enter` resets the view to All and clears project/session search
filters. While filtering, the list shows visible/total counts and the
project panel names the active view. `R` refreshes; it does not clear filters.
Search for `claude`, `codex`, `pi`, or `opencode` to find that agent's rows.

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
| `Enter` | Open the pane, or offer to resume |
| `f` / `p` | Change view / pick project |
| `r` / `d` | Mark to resume / done |
| `n` / `u` | Next-step note / undo |
| `/`, `Ctrl-f`, click the bar | Search projects, names, notes, or text |
| `z` | Expand the conversation |
| `[` / `]` | Older / newer pages |
| `b` / `t` | Branches / tool output |
| `o` / `i` | Browse open panes / file info |
| `gg`, `G`, `Ctrl-d/u` | Jump to ends, half pages |
| `Esc`, `?`, `q` | Back, actions, quit (monitor only) |

## CLI reference

```sh
agent-monitor sessions --all --project acme-website
agent-monitor sessions --open --json
agent-monitor agent explain --file screen.txt --agent codex
agent-monitor show <id>
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

## Live agent activity (no plugin)

Every pane running a recognized agent (Pi, Claude, Codex, OpenCode)
appears in the main All/Open list, the `o` picker, and `sessions --open`,
with a screen-inferred state word (`working/idle/blocked/unknown`). Detection reads the live
bottom of the pane (`tmux capture-pane`, read-only) and matches it
against bundled per-agent manifests; `blocked` requires a known approval
marker, and unmatched output falls back to `idle` (or `unknown` for
Codex without a title signal). Captured text is matched in memory and
dropped — never logged, never stored.

`agent explain` shows the provenance behind any classification:

```sh
agent-monitor agent explain %3 --json
agent-monitor agent explain --file screen.txt --agent codex
```

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
