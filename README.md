# agent-monitor

**Never lose a Pi conversation again.**

A lazygit-style terminal explorer for your Pi sessions: projects on the
left, conversations in the middle, full preview on the right. No task
boards, no tickets, no setup ritual. Open it, find the conversation,
pick up exactly where you left off.

```sh
cargo build --release --locked
./target/release/agent-monitor
```

## Why you'll like it

- **Zero setup.** Projects, names, and history are discovered from your
  Pi files automatically. No extension required.
- **Real names, real context.** `Acme Website / website-redesign` — the
  project and the conversation, side by side, never a bare cryptic id.
- **Resume in one keystroke.** `Enter` jumps to the live pane, or reopens
  the *same* conversation in a fresh tmux window when the old pane is gone.
- **Your decisions stick.** `To resume` and `Done` are yours alone. Idle
  panes, closed terminals, and slow imports never flip them.
- **Honest presence.** Verified openings (`●`), probable hints (`≈`),
  and unverified runs are shown for what they are — never guessed.
- **Keyboard-first, mouse-friendly.** Arrows, `/` search, `?` menu, plus
  click and scroll where you'd expect them.

## 30-second tour

It opens in **All**: every project and conversation Pi ever saved, with an
always-visible search bar on top (`/`, `Ctrl-f`, or click). Search matches
names, folders, and your own notes. Picking a filtered project with `Enter`
dives straight in.

Sessions marked **`[R]`** want your attention — that's the to-resume list
(`f` switches views: All, To resume, Open, Done). `●` means a verified
open pane, `≈` a probable one; both are about *where Pi runs*, while `[R]`
is about *what you want to finish*.

`Enter` on a session does the obvious thing: go to its pane, pick between
openings, or — when nothing is open — ask to reopen that exact JSONL with
`pi --session` in a new tmux window. Existing panes are never touched.

`?` opens a contextual action menu: one readable row per action, shortcut
aligned right, plain explanation below. Arrows + `Enter`, or click.

## Keys

| Key | Does |
|---|---|
| `j/k`, arrows | Move or scroll |
| `h/l`, `Tab` | Switch panel |
| `Enter` | Open the pane, or offer to resume |
| `f` / `p` | Change view / pick project |
| `r` / `d` | To resume / done |
| `n` / `u` | Next-step note / undo |
| `/`, `Ctrl-f`, click the bar | Search projects, names, notes, or text |
| `z` | Expand the conversation |
| `[` / `]` | Older / newer pages |
| `b` / `t` | Branches / tool output |
| `o` / `i` | Browse open panes / file info |
| `gg`, `G`, `Ctrl-d/u` | Jump to ends, half pages |
| `Esc`, `?`, `q` | Back, actions, quit (monitor only) |

Notes save when you see **Saved**. Quitting (`q`) only ever quits the
monitor — never Pi, never tmux.

## Optional: exact presence

Without anything installed, open panes are matched by exact title + folder
and shown as probable (`≈`). Duplicate names are never guessed. For proven
identity (`●`) — even across `/resume` — install the tiny local extension:

```sh
./target/release/agent-monitor integration pi install
```

Then run `/reload` in every Pi that's already open. New Pi runs pick it up
automatically. The monitor never sends `/reload`, prompts, or permission
answers on your behalf.

## Scripting

```sh
agent-monitor sessions --all --project acme-website
agent-monitor sessions --open --json
agent-monitor show <id>
agent-monitor done <id>
agent-monitor reopen <id>
agent-monitor note <id> 'Check the timeout'
agent-monitor undo <id>
agent-monitor sync
agent-monitor sources list
agent-monitor sources add /path/to/sessions
agent-monitor integration pi status
```

`<id>` is the Pi id or the catalog id from `--json`; ambiguous ids are
rejected. `open <id>` jumps to a verified opening. Resuming a closed Pi
asks first (or pass `--resume` in scripts) and rechecks identity, folder,
and presence immediately before launch.

## How it works, briefly

- Rust + SQLite + Ratatui, macOS and Linux. Reads Pi JSONL **v3**
  read-only — transcripts stay the source of truth.
- Private state in `~/.local/state/agent-monitor/sessions.db` (dir 0700,
  db 0600). Override with `AGENT_MONITOR_HOME` or `--data-dir`.
- First import stays history; verified opens and genuinely new sessions
  join To resume. `Done` survives reimports, restarts, and undo history.
- Paginated previews (100 per page), branch picker, optional tool output.
  Thinking blocks and images are always hidden. Oversized files are
  reported, not swallowed.
- Resume uses exact argv (`pi --session <file>`, in the project folder),
  preferring a new window in the project's tmux session, else a fresh
  detached session. No shell interpolation, no closed panes, no config edits.
- No network, no telemetry, no credentials read.

## Checks

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

Tests use temp dirs and fake `pi`/`tmux` executables — no real provider
ever starts. Load numbers are local measurements, not promises.

MIT licensed.
