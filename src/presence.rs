//! Optional Pi extension bridge: verifiable openings, nothing more.
use crate::{
    model::{Binding, Bridge},
    paths, pi, tmux,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const EXTENSION: &str = include_str!("../integrations/pi/session-presence.ts");
const MARKER: &str = "/** tmux-agent-monitor presence v1:";
const LEGACY_MARKER: &str = "/** agent-monitor presence v1:";

fn raw_record(path: &Path) -> Result<Bridge> {
    paths::private_file(path)?;
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(
        f.metadata()?.is_file() && f.metadata()?.len() <= 16_384,
        "presence record is too large"
    );
    let mut data = String::new();
    f.take(16_385).read_to_string(&mut data)?;
    Ok(serde_json::from_str(&data)?)
}
pub fn read(path: &Path) -> Result<Bridge> {
    let bridge = raw_record(path)?;
    ensure!(
        bridge.version == 1
            && !bridge.nonce.is_empty()
            && bridge.nonce.len() < 128
            && bridge.pid > 0,
        "invalid presence record"
    );
    let age = paths::now() - bridge.seen;
    ensure!((0..8000).contains(&age), "presence record expired");
    ensure!(
        tmux::process_start(bridge.pid)? == bridge.process_start,
        "Pi instance is gone"
    );
    if let Some(file) = &bridge.file {
        ensure!(file.is_absolute(), "session path is not absolute");
        if file.exists() {
            ensure!(
                pi::header(file)?.native_id == bridge.native_id,
                "Pi file identity changed"
            );
        }
    }
    Ok(bridge)
}

pub fn records(root: &Path, panes: &[tmux::PaneIdentity]) -> Result<Vec<Binding>> {
    let dir = root.join("presence");
    if !dir.exists() {
        return Ok(vec![]);
    }
    paths::secure_dir(&dir)?;
    let mut result = vec![];
    for entry in fs::read_dir(dir)?.take(2048) {
        let path = entry?.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let bridge = match read(&path) {
            Ok(bridge) => bridge,
            Err(_) => {
                let raw = raw_record(&path)?;
                if tmux::process_start(raw.pid).ok().as_deref() == Some(&raw.process_start) {
                    anyhow::bail!("a live Pi has an unverifiable record: run /reload");
                }
                continue;
            }
        };
        let pane = panes
            .iter()
            .find(|p| {
                bridge
                    .socket
                    .as_ref()
                    .is_some_and(|s| tmux::socket_key(s) == tmux::socket_key(&p.socket))
                    && bridge.pane.as_deref() == Some(&p.pane)
                    && tmux::contains_process(p, bridge.pid, &bridge.process_start).unwrap_or(false)
            })
            .map(|p| {
                let mut p = p.clone();
                p.client_pid = bridge.pid;
                p.client_start = bridge.process_start.clone();
                p
            });
        ensure!(
            bridge.pane.is_none() || pane.is_some(),
            "Pi pane is not verifiable"
        );
        result.push(Binding {
            bridge,
            record: path,
            pane,
        });
    }
    Ok(result)
}

/// Move to a verified opening after rechecking OS identity and session.
/// A `/resume` inside the same process rotates nonce/generation, so stale
/// bindings fail closed here instead of jumping to the wrong conversation.
pub fn focus(binding: &Binding) -> Result<()> {
    let current = read(&binding.record)?;
    ensure!(
        current.nonce == binding.bridge.nonce
            && current.generation == binding.bridge.generation
            && current.native_id == binding.bridge.native_id
            && current.file == binding.bridge.file,
        "Pi switched sessions; refresh the list"
    );
    let pane = binding
        .pane
        .as_ref()
        .context("Pi is open outside tmux; return to its own terminal")?;
    tmux::focus(pane, &tmux::TmuxConfig::default())
}

/// Where the extension lives inside the user's Pi profile.
pub fn installed_path() -> PathBuf {
    extension_path(&paths::pi_dir())
}
fn extension_path(profile: &Path) -> PathBuf {
    let legacy = profile.join("extensions/agent-monitor-presence.ts");
    if fs::symlink_metadata(&legacy).is_ok() {
        legacy
    } else {
        profile.join("extensions/tmux-agent-monitor-presence.ts")
    }
}

/// Install our own extension file atomically (0600), never touching
/// unrelated extensions. An existing foreign file is refused, not replaced.
pub fn install() -> Result<PathBuf> {
    let path = installed_path();
    let parent = path.parent().unwrap();
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let stat = fs::symlink_metadata(parent)?;
    ensure!(
        stat.is_dir() && !stat.file_type().is_symlink() && stat.uid() == unsafe { libc::geteuid() },
        "extension directory is unsafe"
    );
    if fs::symlink_metadata(&path).is_ok() {
        owned_extension(&path)?;
    }
    let tmp = parent.join(format!(".tmux-agent-monitor-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(EXTENSION.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result?;
    Ok(path)
}
/// Accept only extension files we own: regular file, same user,
/// non-symlink, and carrying our marker header.
fn owned_extension(path: &Path) -> Result<()> {
    let stat = fs::symlink_metadata(path)?;
    ensure!(
        stat.is_file()
            && !stat.file_type().is_symlink()
            && stat.uid() == unsafe { libc::geteuid() },
        "extension file is unsafe"
    );
    let text = fs::read_to_string(path)?;
    ensure!(
        text.starts_with(MARKER) || text.starts_with(LEGACY_MARKER),
        "refusing to overwrite a foreign extension"
    );
    Ok(())
}
pub fn remove() -> Result<PathBuf> {
    let path = installed_path();
    owned_extension(&path)?;
    fs::remove_file(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Bridge;
    #[test]
    fn rename_reuses_owned_legacy_observer_without_installing_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            extension_path(dir.path()),
            dir.path().join("extensions/tmux-agent-monitor-presence.ts")
        );
        let legacy = dir.path().join("extensions/agent-monitor-presence.ts");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, format!("{LEGACY_MARKER} fixture */")).unwrap();
        assert_eq!(extension_path(dir.path()), legacy);
        assert!(owned_extension(&legacy).is_ok());
        fs::write(&legacy, EXTENSION).unwrap();
        assert!(owned_extension(&legacy).is_ok());
        fs::write(&legacy, "foreign observer").unwrap();
        assert!(owned_extension(&legacy).is_err());
    }
    fn fixture() -> (tempfile::TempDir, PathBuf, Bridge) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        paths::secure_dir(&root).unwrap();
        let record = root.join("pi.json");
        let bridge = Bridge {
            version: 1,
            nonce: "instance-one".into(),
            generation: 1,
            pid: std::process::id(),
            process_start: tmux::process_start(std::process::id()).unwrap(),
            native_id: "session-one".into(),
            file: None,
            cwd: "/fixture".into(),
            name: "name".into(),
            leaf: None,
            socket: None,
            pane: None,
            seen: paths::now(),
        };
        save(&record, &bridge);
        (dir, record, bridge)
    }
    fn save(path: &Path, bridge: &Bridge) {
        fs::write(path, serde_json::to_vec(bridge).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn expiry_and_pid_reuse_are_not_presence() {
        let (_dir, record, mut bridge) = fixture();
        assert!(read(&record).is_ok());
        bridge.seen -= 9000;
        save(&record, &bridge);
        assert!(read(&record).is_err());
        bridge.seen = paths::now();
        bridge.process_start = "wrong-start-token".into();
        save(&record, &bridge);
        assert!(read(&record).is_err());
    }
    #[test]
    fn switching_session_in_same_process_invalidates_focus() {
        let (_dir, record, mut bridge) = fixture();
        let binding = Binding {
            bridge: bridge.clone(),
            record: record.clone(),
            pane: None,
        };
        bridge.native_id = "session-two".into();
        bridge.generation += 1;
        save(&record, &bridge);
        let error = focus(&binding).unwrap_err().to_string();
        assert!(error.contains("switched sessions"));
    }
    #[test]
    fn observer_symlinks_are_refused() {
        let (_dir, record, _) = fixture();
        let link = record.with_extension("link");
        std::os::unix::fs::symlink(&record, &link).unwrap();
        assert!(read(&link).is_err());
    }
}
