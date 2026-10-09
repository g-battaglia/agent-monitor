//! Private local storage, safe display strings, and bounded subprocesses.
//!
//! The state directory and database are created owner-only; symlinks and
//! group/other-readable files are refused. Display helpers strip ANSI,
//! control, and bidi-spoofing characters before anything reaches the UI.
use anyhow::{Result, ensure};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

/// Explicit --data-dir, canonical environment variable, then legacy alias.
/// Reuse existing legacy state without moving files or splitting presence.
/// Only sessions.db is used; board.db/library.db are never opened.
pub fn root(data_dir: Option<PathBuf>) -> Result<PathBuf> {
    let root = data_dir
        .or_else(|| std::env::var_os("TMUX_AGENT_MONITOR_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("AGENT_MONITOR_HOME").map(PathBuf::from))
        .unwrap_or_else(|| default_root(&home()));
    let root = expand_home(&root);
    ensure!(root.is_absolute(), "data directory must be absolute");
    Ok(root)
}

fn default_root(home: &Path) -> PathBuf {
    let canonical = home.join(".local/state/tmux-agent-monitor");
    let legacy = home.join(".local/state/agent-monitor");
    if fs::symlink_metadata(canonical.join("sessions.db")).is_ok() {
        canonical
    } else if fs::symlink_metadata(&legacy).is_ok() {
        legacy
    } else {
        canonical
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Ensure a private `0700` directory owned by the current user.
/// Existing directories are rechecked; symlinks are always refused.
pub fn secure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.is_dir(),
        "not a private directory: {}",
        path.display()
    );
    use std::os::unix::fs::MetadataExt;
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
        "directory must be owned by you and mode 0700: {}",
        path.display()
    );
    Ok(())
}

/// Refuse group/other-readable files and symlinks.
/// Called before opening the database, bridge records, and the DB path
/// itself when it already exists.
pub fn private_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    use std::os::unix::fs::MetadataExt;
    ensure!(
        !metadata.file_type().is_symlink()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "unsafe file: {}",
        path.display()
    );
    Ok(())
}

/// Strip ANSI escapes, OSC sequences, control characters, and Unicode
/// bidi-spoofing marks. Newlines and tabs survive for readable layout.
pub fn clean(text: &str) -> String {
    String::from_utf8_lossy(&strip_ansi_escapes::strip(text.as_bytes()))
        .chars()
        .filter(|c| (!c.is_control() || *c == '\n' || *c == '\t')
            && !matches!(*c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect()
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub fn date(time: i64) -> String {
    if time <= 0 {
        return "—".into();
    }
    chrono::DateTime::from_timestamp_millis(time)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%d/%m %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}
pub fn line(text: &str) -> String {
    clean(text).split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn expand_home(path: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map(|tail| home().join(tail))
        .unwrap_or_else(|_| path.to_path_buf())
}
pub fn canonical(path: &Path) -> PathBuf {
    let path = expand_home(path);
    fs::canonicalize(&path).unwrap_or(path)
}
/// Run one subprocess with capped output and a hard timeout.
/// Dead tmux servers fail fast instead of stalling the worker thread.
pub fn output(command: &mut std::process::Command) -> Result<std::process::Output> {
    use std::{
        io::Read,
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(8 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("subprocess timed out");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(std::process::Output {
        status,
        stdout: out
            .join()
            .map_err(|_| anyhow::anyhow!("stdout reader failed"))??,
        stderr: err
            .join()
            .map_err(|_| anyhow::anyhow!("stderr reader failed"))??,
    })
}

pub fn pi_dir() -> PathBuf {
    canonical(
        &std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".pi/agent")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rename_preserves_legacy_state_without_moving_or_opening_databases() {
        let home = tempfile::tempdir().unwrap();
        let fresh = home.path().join(".local/state/tmux-agent-monitor");
        assert_eq!(default_root(home.path()), fresh);
        let legacy = home.path().join(".local/state/agent-monitor");
        secure_dir(&legacy).unwrap();
        fs::write(legacy.join("sessions.db"), b"fixture").unwrap();
        assert_eq!(default_root(home.path()), legacy);
        assert_eq!(fs::read(legacy.join("sessions.db")).unwrap(), b"fixture");
        secure_dir(&fresh).unwrap();
        assert_eq!(default_root(home.path()), legacy);
        fs::write(fresh.join("sessions.db"), b"new fixture").unwrap();
        assert_eq!(default_root(home.path()), fresh);
        fs::remove_file(fresh.join("sessions.db")).unwrap();
        fs::remove_dir_all(&legacy).unwrap();
        std::os::unix::fs::symlink(&fresh, &legacy).unwrap();
        assert_eq!(default_root(home.path()), legacy);
        assert!(secure_dir(&default_root(home.path())).is_err());
    }
    #[test]
    fn terminal_controls_and_directional_spoofing_are_removed() {
        let raw = "safe\x1b]0;title\x07\x1b[31mtext\x1b[0m\u{202e}hidden\nnext";
        let value = clean(raw);
        assert!(!value.contains('\x1b'));
        assert!(!value.contains('\u{202e}'));
        assert_eq!(line(&value), "safetexthidden next");
    }
}
