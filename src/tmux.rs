//! Read-only tmux discovery plus explicit, revalidated navigation.
//!
//! Discovery only lists panes and matches same-user agent processes.
//! Moving to a pane, or opening a fresh window/session for resume,
//! always happens after an explicit user confirmation. This module never
//! closes panes, kills processes, or edits tmux configuration.
use crate::{model::Provider, paths::clean};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, process::Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneIdentity {
    pub socket: String,
    pub server: String,
    pub pane: String,
    pub pane_pid: u32,
    pub pane_start: String,
    pub client_pid: u32,
    pub client_start: String,
    pub target: String,
    pub session: String,
    pub window: String,
    pub cwd: String,
    pub command: String,
    #[serde(default)]
    pub title: String,
    pub provider: Option<Provider>,
}

#[derive(Debug, Clone, Default)]
pub struct TmuxConfig {
    pub sockets: Vec<String>,
}

/// One same-user OS process, with the fields needed for ancestry checks.
struct Proc {
    parent: u32,
    start: String,
    command: String,
}

/// Run one bounded `tmux -S <socket> …` command and return stdout.
fn tmux(socket: &str, args: &[&str]) -> Result<String> {
    let output = crate::paths::output(Command::new("tmux").args(["-S", socket]).args(args))?;
    ensure!(
        output.status.success(),
        "tmux: {}",
        clean(&String::from_utf8_lossy(&output.stderr))
    );
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Canonicalize socket aliases (`/tmp` versus `/private/tmp` on macOS).
/// Comparisons elsewhere must use this key, not the raw env string.
pub fn socket_key(socket: &str) -> String {
    std::fs::canonicalize(socket)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| socket.to_string())
}

/// Snapshot same-user processes once per discovery pass.
/// Other users are ignored before any matching happens.
fn processes() -> Result<HashMap<u32, Proc>> {
    let output = crate::paths::output(
        Command::new("ps")
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .args(["-axo", "pid=,ppid=,uid=,lstart=,comm="]),
    )?;
    ensure!(output.status.success(), "cannot inspect processes");
    let mut result = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 9 || parts[2].parse::<u32>().ok() != Some(unsafe { libc::geteuid() }) {
            continue;
        }
        if let (Ok(pid), Ok(parent)) = (parts[0].parse(), parts[1].parse()) {
            result.insert(
                pid,
                Proc {
                    parent,
                    start: parts[3..8].join(" "),
                    command: parts[8..].join(" "),
                },
            );
        }
    }
    Ok(result)
}

/// Ancestor depth from `pid` up to `root`, capped to avoid loops.
/// Used to decide whether an agent process belongs to a given pane.
fn distance(pid: u32, root: u32, all: &HashMap<u32, Proc>) -> Option<usize> {
    let mut current = pid;
    for depth in 0..64 {
        if current == root {
            return Some(depth);
        }
        let proc = all.get(&current)?;
        if proc.parent == current {
            return None;
        }
        current = proc.parent;
    }
    None
}

/// Recognize supported agents by exact executable basename.
/// Substring matches such as `not-codex` must never count as an agent.
fn provider(command: &str) -> Option<Provider> {
    match Path::new(command).file_name()?.to_str()? {
        "pi" | "pi-coding-agent" => Some(Provider::Pi),
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        "opencode" => Some(Provider::Opencode),
        _ => None,
    }
}

/// Which tmux servers to query: explicit config first, otherwise the
/// current `$TMUX` socket plus every socket in the user tmp directory.
/// Results are canonicalized, sorted, and deduplicated.
pub fn sockets(config: &TmuxConfig) -> Vec<String> {
    if !config.sockets.is_empty() {
        let mut sockets: Vec<_> = config.sockets.iter().map(|s| socket_key(s)).collect();
        sockets.sort();
        sockets.dedup();
        return sockets;
    }
    let mut sockets = vec![];
    if let Ok(tmux) = std::env::var("TMUX")
        && let Some((value, _)) = tmux.rsplit_once(',')
        && let Some((path, _)) = value.rsplit_once(',')
    {
        sockets.push(path.to_string());
    }
    let uid = unsafe { libc::geteuid() };
    let root = std::env::var_os("TMUX_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into())
        .join(format!("tmux-{uid}"));
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            use std::os::unix::fs::FileTypeExt;
            if !entry.file_type().is_ok_and(|t| t.is_socket()) {
                continue;
            }
            if let Some(path) = entry.path().to_str() {
                sockets.push(path.to_string());
            }
        }
    }
    let mut sockets: Vec<_> = sockets.into_iter().map(|s| socket_key(&s)).collect();
    sockets.sort();
    sockets.dedup();
    sockets
}

/// List every pane on every observed tmux server.
///
/// For each pane shell, the nearest descendant agent process wins over
/// nested helpers, so a wrapper around `pi` does not hide the real tool.
/// Panes whose shell has no agent descendant keep the pane command as a
/// fallback guess for the provider field.
pub fn discover(config: &TmuxConfig) -> Result<Vec<PaneIdentity>> {
    let all = processes()?;
    let mut panes = vec![];
    for socket in sockets(config) {
        let server = match tmux(&socket, &["display-message", "-p", "#{pid}:#{start_time}"]) {
            Ok(server) => server.trim().to_string(),
            Err(_) => anyhow::bail!(
                "tmux server is not verifiable: {}",
                crate::paths::line(&socket)
            ),
        };
        let output = tmux(
            &socket,
            &[
                "list-panes",
                "-a",
                "-F",
                "#{pane_id}\t#{pane_pid}\t#{pane_current_command}\t#{pane_current_path}\t#{session_name}\t#{window_index}\t#{session_name}:#{window_index}.#{pane_index}\t#{pane_title}",
            ],
        )?;
        for line in output.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 8 {
                continue;
            }
            let Ok(pid) = fields[1].parse::<u32>() else {
                continue;
            };
            let Some(root) = all.get(&pid) else {
                continue;
            };
            let mut identified: Vec<_> = all
                .iter()
                .filter(|(id, proc)| {
                    distance(**id, pid, &all).is_some() && provider(&proc.command).is_some()
                })
                .collect();
            identified
                .sort_by_key(|(id, _)| (distance(**id, pid, &all).unwrap_or(usize::MAX), **id));
            let (client_pid, client_start, tool) = identified
                .first()
                .map(|(id, proc)| (**id, proc.start.clone(), provider(&proc.command)))
                .unwrap_or((pid, root.start.clone(), provider(fields[2])));
            panes.push(PaneIdentity {
                socket: socket.clone(),
                server: server.clone(),
                pane: fields[0].to_string(),
                pane_pid: pid,
                pane_start: root.start.clone(),
                client_pid,
                client_start,
                target: clean(fields[6]),
                session: fields[4].to_string(),
                window: fields[5].to_string(),
                cwd: fields[3].to_string(),
                command: clean(fields[2]),
                title: clean(fields[7]),
                provider: tool,
            });
        }
    }
    Ok(panes)
}

/// Count live Pi processes with no extension record.
/// Nonzero means some openings can only be guessed, never proven.
pub fn unreported_pi(reported: &[u32]) -> Result<usize> {
    Ok(processes()?
        .iter()
        .filter(|(pid, p)| provider(&p.command) == Some(Provider::Pi) && !reported.contains(pid))
        .count())
}

/// True when `pid` (with this exact start time) is still a descendant
/// of the recorded pane shell. Catches PID reuse across restarts.
pub fn contains_process(pane: &PaneIdentity, pid: u32, start: &str) -> Result<bool> {
    let all = processes()?;
    Ok(all.get(&pid).is_some_and(|p| p.start == start)
        && distance(pid, pane.pane_pid, &all).is_some())
}

/// `lstart` plus UID check for one process.
/// Rejects missing processes and other users' PIDs outright.
pub fn process_start(pid: u32) -> Result<String> {
    let output = crate::paths::output(
        Command::new("ps")
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .args(["-p", &pid.to_string(), "-o", "lstart=,uid="]),
    )?;
    let text = String::from_utf8_lossy(&output.stdout);
    let parts = text.split_whitespace().collect::<Vec<_>>();
    ensure!(
        output.status.success()
            && parts.len() == 6
            && parts[5].parse::<u32>().ok() == Some(unsafe { libc::geteuid() }),
        "process is gone or belongs to another user"
    );
    Ok(parts[..5].join(" "))
}

/// Revalidate immediately before moving: same server, socket, pane,
/// pane shell, agent PID, and both process start tokens.
pub fn valid(identity: &PaneIdentity, config: &TmuxConfig) -> bool {
    if process_start(identity.client_pid).ok().as_deref() != Some(&identity.client_start) {
        return false;
    }
    discover(config).ok().is_some_and(|panes| {
        panes.iter().any(|pane| {
            socket_key(&pane.socket) == socket_key(&identity.socket)
                && pane.server == identity.server
                && pane.pane == identity.pane
                && pane.pane_pid == identity.pane_pid
                && pane.pane_start == identity.pane_start
                && contains_process(identity, identity.client_pid, &identity.client_start)
                    .unwrap_or(false)
        })
    })
}

/// Move to a probable pane after rechecking title, cwd, and processes.
///
/// This is explicitly pane navigation, not proof of which conversation
/// the process holds. Renamed panes and reused PIDs are rejected.
pub fn focus_probable(identity: &PaneIdentity) -> Result<()> {
    let config = TmuxConfig {
        sockets: vec![identity.socket.clone()],
    };
    let panes = discover(&config)?;
    ensure!(
        panes.iter().any(|p| same_hint(p, identity)),
        "The pane or its title changed: refresh and choose again"
    );
    focus(identity, &config)
}
fn same_hint(current: &PaneIdentity, selected: &PaneIdentity) -> bool {
    socket_key(&current.socket) == socket_key(&selected.socket)
        && current.server == selected.server
        && current.pane == selected.pane
        && current.client_pid == selected.client_pid
        && current.client_start == selected.client_start
        && current.title == selected.title
        && current.cwd == selected.cwd
        && current.provider == Some(Provider::Pi)
}

pub fn focus(identity: &PaneIdentity, config: &TmuxConfig) -> Result<()> {
    ensure!(
        valid(identity, config),
        "Opening is no longer verifiable; wait for the bridge or run /reload"
    );
    // Switch only the caller's own tmux client, never another attached client.
    let own = std::env::var("TMUX").context("Run the monitor inside tmux to jump to a pane")?;
    let own_socket = own
        .rsplit_once(',')
        .and_then(|(value, _)| value.rsplit_once(','))
        .map(|(socket, _)| socket_key(socket));
    ensure!(
        own_socket.as_deref() == Some(socket_key(&identity.socket).as_str()),
        "pane belongs to another server; attach manually with tmux -S {} attach",
        identity.socket
    );
    let monitor_pane = std::env::var("TMUX_PANE").context("Monitor pane is not identifiable")?;
    let session = tmux(
        &identity.socket,
        &[
            "display-message",
            "-p",
            "-t",
            &monitor_pane,
            "#{session_name}",
        ],
    )?;
    let clients = tmux(
        &identity.socket,
        &["list-clients", "-F", "#{client_tty}\t#{client_session}"],
    )?;
    let hint = std::env::var("AGENT_MONITOR_TMUX_CLIENT").ok();
    let client = requesting_client(&clients, session.trim(), hint.as_deref())?;
    tmux(
        &identity.socket,
        &["switch-client", "-c", &client, "-t", &identity.pane],
    )?;
    tmux(&identity.socket, &["select-pane", "-t", &identity.pane])?;
    Ok(())
}

/// Resume the SAME Pi JSONL in a fresh detached window/session, then reach it.
///
/// Prefers a new window in the one tmux session already rooted in the
/// project directory; otherwise creates a fresh detached session with a
/// unique name. Every tmux invocation passes argv directly (execvp):
/// file paths and titles are never interpolated into a shell command.
pub fn resume(file: &Path, cwd: &Path, title: &str) -> Result<()> {
    let socket = std::env::var("TMUX").ok().and_then(|v| {
        v.rsplit_once(',')
            .and_then(|(v, _)| v.rsplit_once(','))
            .map(|(s, _)| s.to_owned())
    });
    let monitor = std::env::var("TMUX_PANE").ok();
    let opened = resume_with(
        std::ffi::OsStr::new("tmux"),
        socket.as_deref(),
        monitor.as_deref(),
        file,
        cwd,
        title,
    )?;
    if let Some(target) = opened {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            println!(
                "Conversation reopened in tmux session {}",
                crate::paths::line(&target)
            );
            return Ok(());
        }
        let mut command = Command::new("tmux");
        if let Some(socket) = socket {
            command.args(["-S", &socket]);
        }
        let status = command.args(["attach-session", "-t", &target]).status()?;
        ensure!(status.success(), "Could not attach to the new tmux session");
    }
    Ok(())
}
fn resume_with(
    program: &std::ffi::OsStr,
    socket: Option<&str>,
    monitor: Option<&str>,
    file: &Path,
    cwd: &Path,
    title: &str,
) -> Result<Option<String>> {
    let run = |args: &[&str]| -> Result<String> {
        let mut command = Command::new(program);
        if let Some(socket) = socket {
            command.args(["-S", socket]);
        }
        let out = crate::paths::output(command.args(args))?;
        ensure!(
            out.status.success(),
            "tmux: {}",
            crate::paths::line(&String::from_utf8_lossy(&out.stderr))
        );
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    // Resolve the requesting client BEFORE creating anything. Two clients
    // on the monitor session is an error, never permission to guess
    // which terminal to move.
    let client = if let Some(pane) = monitor {
        if let Ok(session) = run(&["display-message", "-p", "-t", pane, "#{session_name}"]) {
            Some(requesting_client(
                &run(&["list-clients", "-F", "#{client_tty}\t#{client_session}"])?,
                session.trim(),
                std::env::var("AGENT_MONITOR_TMUX_CLIENT").ok().as_deref(),
            )?)
        } else {
            None
        }
    } else {
        None
    };
    let listing = run(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_id}\t#{pane_current_path}",
    ])
    .unwrap_or_default();
    let mut matching = listing
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, path)| {
            crate::paths::canonical(Path::new(path)) == crate::paths::canonical(cwd)
        })
        .map(|(id, _)| id.to_owned())
        .collect::<Vec<_>>();
    matching.sort();
    matching.dedup();
    let name = crate::model::project_name(&cwd.to_string_lossy())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect::<String>();
    let unique = format!(
        "{}-{}",
        if name.is_empty() { "pi" } else { &name },
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    let file = file.to_str().context("Pi path is not UTF-8")?;
    let cwd = cwd.to_str().context("Project directory is not UTF-8")?;
    let output = if matching.len() == 1 {
        run(&[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}\t#{session_id}",
            "-t",
            &format!("{}:", matching[0]),
            "-n",
            &crate::paths::line(title),
            "-c",
            cwd,
            "pi",
            "--session",
            file,
        ])?
    } else {
        run(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}\t#{session_id}",
            "-s",
            &unique,
            "-n",
            &crate::paths::line(title),
            "-c",
            cwd,
            "pi",
            "--session",
            file,
        ])?
    };
    let (pane, session) = output
        .trim()
        .split_once('\t')
        .context("tmux did not report the new opening")?;
    if let Some(client) = client {
        run(&["switch-client", "-c", &client, "-t", pane])?;
        run(&["select-pane", "-t", pane])?;
        Ok(None)
    } else {
        Ok(Some(session.to_owned()))
    }
}

fn requesting_client(text: &str, session: &str, hint: Option<&str>) -> Result<String> {
    let candidates: Vec<_> = text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, name)| *name == session)
        .map(|(tty, _)| tty)
        .collect();
    if let Some(tty) = hint {
        ensure!(
            candidates.contains(&tty),
            "The requested client is not attached to the monitor tmux session"
        );
        Ok(tty.to_string())
    } else {
        ensure!(
            candidates.len() == 1,
            "requesting tmux client is ambiguous; set AGENT_MONITOR_TMUX_CLIENT to its client_tty"
        );
        Ok(candidates[0].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probable_focus_rejects_reused_pids_and_renamed_panes() {
        let pane = PaneIdentity {
            socket: "/fake/socket".into(),
            server: "1".into(),
            pane: "%1".into(),
            pane_pid: 1,
            pane_start: "start".into(),
            client_pid: 2,
            client_start: "start".into(),
            target: "project:1.0".into(),
            session: "project".into(),
            window: "1".into(),
            cwd: "/repo/project".into(),
            command: "pi".into(),
            title: "π - release-notes - project".into(),
            provider: Some(Provider::Pi),
        };
        assert!(same_hint(&pane, &pane));
        let mut changed = pane.clone();
        changed.title = "π - another - project".into();
        assert!(!same_hint(&changed, &pane));
        changed = pane.clone();
        changed.client_start = "new start".into();
        assert!(!same_hint(&changed, &pane));
        changed = pane.clone();
        changed.cwd = "/repo/elsewhere".into();
        assert!(!same_hint(&changed, &pane));
        changed = pane.clone();
        changed.socket = "/other/server".into();
        assert!(!same_hint(&changed, &pane));
    }
    #[test]
    fn resume_creates_only_new_windows_or_sessions_with_exact_argv() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-tmux");
        fs_fixture(&script);
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cwd = dir.path().join("project with spaces");
        std::fs::create_dir(&cwd).unwrap();
        let file = cwd.join("file;$(not-a-command).jsonl");
        // No server/session: a fresh detached session is created, with the
        // original JSONL as one argv element; attaching belongs to the caller.
        let target = resume_with(
            script.as_os_str(),
            None,
            None,
            &file,
            &cwd,
            "release ; name",
        )
        .unwrap();
        assert_eq!(target.as_deref(), Some("$9"));
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        let commands = log
            .lines()
            .map(|l| serde_json::from_str::<Vec<String>>(l).unwrap())
            .collect::<Vec<_>>();
        let create = commands
            .iter()
            .find(|args| args[0] == "new-session")
            .unwrap();
        assert!(
            create
                .windows(3)
                .any(|w| w == ["pi", "--session", file.to_str().unwrap()])
        );
        assert!(create.contains(&cwd.to_string_lossy().into_owned()));
        assert!(create.contains(&"release ; name".into()));
        assert!(
            !commands
                .iter()
                .any(|args| args[0].starts_with("kill-") || args[0] == "send-keys")
        );
        std::fs::write(dir.path().join("panes"), format!("$1\t{}\n", cwd.display())).unwrap();
        std::fs::write(dir.path().join("log"), "").unwrap();
        resume_with(script.as_os_str(), None, None, &file, &cwd, "release").unwrap();
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        assert!(log.contains("new-window"));
        assert!(!log.contains("new-session"));
        std::fs::write(dir.path().join("clients"), "/dev/fakeA\tmonitor\n").unwrap();
        assert!(
            resume_with(
                script.as_os_str(),
                None,
                Some("%monitor"),
                &file,
                &cwd,
                "release"
            )
            .unwrap()
            .is_none()
        );
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        assert!(log.contains("switch-client"));
        assert!(log.contains("/dev/fakeA"));
        std::fs::write(
            dir.path().join("clients"),
            "/dev/fakeA\tmonitor\n/dev/fakeB\tmonitor\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("log"), "").unwrap();
        assert!(
            resume_with(
                script.as_os_str(),
                None,
                Some("%monitor"),
                &file,
                &cwd,
                "release"
            )
            .is_err()
        );
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        assert!(!log.contains("new-window"));
        assert!(!log.contains("new-session"));
    }
    fn fs_fixture(script: &Path) {
        std::fs::write(
            script,
            r#"#!/usr/bin/env python3
import json,sys
from pathlib import Path
root=Path(__file__).parent
args=sys.argv[1:]
with (root/'log').open('a') as file: file.write(json.dumps(args)+'\n')
if args[0]=='list-panes':
    if (root/'panes').exists(): print((root/'panes').read_text(),end='')
    else: sys.exit(1)
elif args[0] in ('new-session','new-window'): print('%99\t$9')
elif args[0]=='display-message': print('monitor')
elif args[0]=='list-clients': print((root/'clients').read_text(),end='')
elif args[0] in ('switch-client','select-pane'): pass
else: sys.exit(1)
"#,
        )
        .unwrap();
    }
    #[test]
    fn focus_never_guesses_between_attached_clients() {
        let clients = "/dev/ttyA\tboard\n/dev/ttyB\tboard\n/dev/ttyC\tother\n";
        assert!(requesting_client(clients, "board", None).is_err());
        assert_eq!(
            requesting_client(clients, "board", Some("/dev/ttyB")).unwrap(),
            "/dev/ttyB"
        );
        assert!(requesting_client(clients, "board", Some("/dev/ttyC")).is_err());
        assert_eq!(
            requesting_client(clients, "other", None).unwrap(),
            "/dev/ttyC"
        );
    }
}
