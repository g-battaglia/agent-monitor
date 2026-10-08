//! Shared CLI/TUI operations: indexing, catalog assembly, and resume specs.
use crate::{
    model::*,
    paths, pi, presence,
    store::{Source, Store},
    tmux,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Opened state root plus SQLite store.
pub struct Service {
    pub root: PathBuf,
    pub store: Store,
}

/// Incremental scan state.
pub struct Indexer {
    queue: VecDeque<(Source, PathBuf)>,
    sources: Vec<Source>,
    pub warnings: Vec<String>,
    pub initial: bool,
    failed: std::collections::HashSet<PathBuf>,
}
impl Indexer {
    pub fn pending(&self) -> bool {
        !self.queue.is_empty()
    }
}
impl Service {
    pub fn open(root: Option<PathBuf>) -> Result<Self> {
        let root = paths::root(root)?;
        let store = Store::open(&root)?;
        let dir = default_source()?;
        store.add_source(&dir)?;
        let standard = paths::pi_dir().join("sessions");
        if standard.is_dir() {
            store.add_source(&standard)?;
        }
        Ok(Self { root, store })
    }
    pub fn begin_index(&self) -> Result<Indexer> {
        let sources = self.store.sources()?;
        let mut indexer = Indexer {
            queue: VecDeque::new(),
            initial: sources.iter().any(|s| !s.complete),
            sources,
            warnings: vec![],
            failed: Default::default(),
        };
        for source in &indexer.sources {
            match if source.path.is_file() {
                Ok(vec![paths::canonical(&source.path)])
            } else {
                pi::files(&source.path)
            } {
                Ok(mut files) => {
                    let own = std::env::current_dir().ok();
                    files.sort_by_key(|p| {
                        !own.as_ref().is_some_and(|c| {
                            p.to_string_lossy()
                                .contains(&paths::line(&c.to_string_lossy()).replace('/', "-"))
                        })
                    });
                    indexer
                        .queue
                        .extend(files.into_iter().map(|p| (source.clone(), p)));
                }
                Err(_) => {
                    indexer.failed.insert(source.path.clone());
                    indexer.warnings.push(format!(
                        "Unreadable source: {}",
                        paths::line(&source.path.to_string_lossy())
                    ));
                }
            }
        }
        Ok(indexer)
    }
    pub fn index_step(&self, indexer: &mut Indexer) -> Result<()> {
        if let Some((source, path)) = indexer.queue.pop_front() {
            let previous = self.store.cursor(&path)?;
            let recognized = previous.is_some() || pi::is_session(&path).unwrap_or(true);
            let stat = fs::metadata(&path);
            let unchanged = previous
                .as_ref()
                .zip(stat.as_ref().ok())
                .is_some_and(|(c, m)| {
                    c.offset == m.len()
                        && c.size == m.len()
                        && c.inode == m.ino()
                        && c.device == m.dev()
                        && c.mtime == m.mtime() * 1_000_000_000 + m.mtime_nsec()
                });
            if recognized && !unchanged {
                match pi::scan(&path, previous.as_ref()) {
                    Ok((cursor, more)) => {
                        self.store.ingest(&source, &cursor)?;
                        if more {
                            indexer.queue.push_front((source, path));
                        }
                    }
                    Err(_) => {
                        indexer.failed.insert(source.path.clone());
                        indexer.warnings.push(format!(
                            "Unreadable or incomplete session: {}",
                            paths::line(&path.to_string_lossy())
                        ));
                    }
                }
            }
        }
        if !indexer.pending() {
            for source in &indexer.sources {
                if !indexer.failed.contains(&source.path) {
                    self.store.complete(&source.path)?;
                }
            }
            for session in self.store.list()? {
                if !session.metadata.file.exists() {
                    self.store.mark_missing(&session.metadata.file)?;
                }
            }
        }
        Ok(())
    }
    pub fn sync(&self) -> Result<Vec<String>> {
        let mut indexer = self.begin_index()?;
        loop {
            self.index_step(&mut indexer)?;
            if !indexer.pending() {
                break;
            }
        }
        Ok(indexer.warnings)
    }
    pub fn catalog(&self) -> Result<Catalog> {
        let mut warnings = vec![];
        let panes = match tmux::discover(&tmux::TmuxConfig::default()) {
            Ok(p) => p,
            Err(_) => {
                warnings.push("tmux presence is unverified".into());
                vec![]
            }
        };
        let bindings = match presence::records(&self.root, &panes) {
            Ok(b) => b,
            Err(e) => {
                warnings.push(format!(
                    "Pi presence is unverified: {}",
                    paths::line(&e.to_string())
                ));
                vec![]
            }
        };
        // Project settings reveal additional storage roots, not session identity.
        for cwd in panes
            .iter()
            .filter(|p| p.provider == Some(Provider::Pi))
            .map(|p| &p.cwd)
            .collect::<std::collections::BTreeSet<_>>()
        {
            if let Some(dir) = project_source(Path::new(cwd)) {
                self.store.add_source(&dir)?;
            }
        }
        // External --session/--session-dir files are exact sources, never cwd guesses.
        for binding in &bindings {
            if let Some(file) = &binding.bridge.file
                && file.exists()
            {
                let path = paths::canonical(file);
                if self.store.cursor(&path)?.is_none() {
                    self.store.add_source(&path)?;
                    let source = self
                        .store
                        .sources()?
                        .into_iter()
                        .find(|s| s.path == path)
                        .context("sorgente mancante")?;
                    if let Ok((cursor, _)) = pi::scan(&path, None) {
                        self.store.ingest(&source, &cursor)?;
                    }
                }
                self.store.manage(&path)?;
            }
        }
        if tmux::unreported_pi(&bindings.iter().map(|b| b.bridge.pid).collect::<Vec<_>>())? > 0 {
            warnings
                .push("Some Pi runs have no extension: probable openings are marked [~]".into());
        }
        let mut sessions = self.store.list()?;
        for session in &mut sessions {
            session.presence_verified = warnings.is_empty();
            session.bindings = bindings
                .iter()
                .filter(|b| {
                    b.bridge.native_id == session.metadata.native_id
                        && b.bridge
                            .file
                            .as_ref()
                            .is_some_and(|p| paths::canonical(p) == session.metadata.file)
                })
                .cloned()
                .collect();
            session.live_name();
        }
        for binding in &bindings {
            if !sessions.iter().any(|s| {
                s.bindings
                    .iter()
                    .any(|b| b.bridge.nonce == binding.bridge.nonce)
            }) {
                sessions.push(Session {
                    id: format!("live-{}", binding.bridge.nonce),
                    metadata: Metadata {
                        native_id: binding.bridge.native_id.clone(),
                        file: binding.bridge.file.clone().unwrap_or_default(),
                        cwd: binding.bridge.cwd.clone(),
                        name: binding.bridge.name.clone(),
                        warning: "Session not saved by Pi yet (or --no-session)".into(),
                        ..Default::default()
                    },
                    state: ResumeState::Resume,
                    note: String::new(),
                    done_revision: 0,
                    done_at: 0,
                    available: false,
                    presence_verified: true,
                    bindings: vec![binding.clone()],
                    probable: vec![],
                });
            }
        }
        let unbound = panes
            .into_iter()
            .filter(|p| {
                p.provider == Some(Provider::Pi)
                    && !bindings.iter().any(|b| {
                        b.pane
                            .as_ref()
                            .is_some_and(|bp| bp.socket == p.socket && bp.pane == p.pane)
                    })
            })
            .collect();
        let unbound: Vec<tmux::PaneIdentity> = unbound;
        if !unbound.is_empty() {
            for session in &mut sessions {
                session.presence_verified = false;
            }
        }
        associate_probable(&mut sessions, &unbound);
        Ok(Catalog {
            sessions,
            unbound,
            warnings,
            scanning: false,
        })
    }
    pub fn detail(&self, target: &str, leaf: Option<&str>, tools: bool) -> Result<Conversation> {
        self.detail_page(target, leaf, tools, 0)
    }
    pub fn detail_page(
        &self,
        target: &str,
        leaf: Option<&str>,
        tools: bool,
        page: usize,
    ) -> Result<Conversation> {
        let session = self.store.resolve(target)?;
        ensure!(session.available, "conversation is unavailable");
        let current = pi::header(&session.metadata.file)?;
        ensure!(
            current.native_id == session.metadata.native_id,
            "session identity changed"
        );
        pi::conversation_page(&session.metadata.file, leaf, tools, page)
    }
    pub fn resolve<'a>(&self, catalog: &'a Catalog, target: &str) -> Result<&'a Session> {
        let matches = catalog
            .sessions
            .iter()
            .filter(|s| s.id == target || s.metadata.native_id == target)
            .collect::<Vec<_>>();
        ensure!(matches.len() <= 1, "ambiguous id: choose the exact session");
        matches.into_iter().next().context("session not found")
    }
    /// Build a resume plan without starting anything. The UI/CLI shows it,
    /// asks for confirmation, then rebuilds and rechecks it right before launch.
    pub fn resume_spec(&self, target: &str) -> Result<ResumeSpec> {
        let catalog = self.catalog()?;
        let session = self.resolve(&catalog, target)?;
        ensure!(
            session.bindings.is_empty(),
            "session is already open: use the existing pane"
        );
        ensure!(
            session.probable.is_empty(),
            "Probable opening: pick the pane in the monitor instead of starting a possible duplicate"
        );
        // Other unverified Pi processes must not prevent resuming unrelated
        // work. The uncertainty is shown before an explicit confirmation.
        let warning = if !catalog.unbound.is_empty() || !catalog.warnings.is_empty() {
            "Some Pi runs are unidentified: another opening of this conversation cannot be ruled out.".into()
        } else {
            String::new()
        };
        ensure!(
            session.available && session.metadata.file.is_file(),
            "session file is unavailable"
        );
        ensure!(
            Path::new(&session.metadata.cwd).is_dir(),
            "project directory is unavailable"
        );
        ensure!(
            pi::header(&session.metadata.file)?.native_id == session.metadata.native_id,
            "Pi identity changed"
        );
        Ok(ResumeSpec {
            file: session.metadata.file.clone(),
            cwd: session.metadata.cwd.clone(),
            title: session.metadata.title(),
            native_id: session.metadata.native_id.clone(),
            warning,
        })
    }
}
/// Attach passive title/cwd hints. Exact unique matches only; duplicates
/// stay unassociated and stored state is never changed.
pub fn associate_probable(sessions: &mut [Session], panes: &[tmux::PaneIdentity]) {
    for session in sessions.iter_mut() {
        session.probable.clear();
    }
    for pane in panes.iter().filter(|p| p.provider == Some(Provider::Pi)) {
        let cwd = paths::canonical(Path::new(&pane.cwd))
            .to_string_lossy()
            .into_owned();
        let title = paths::line(&pane.title);
        let Some(name) = title
            .strip_prefix("π - ")
            .or_else(|| title.strip_prefix("pi - "))
            .and_then(|s| s.strip_suffix(&format!(" - {}", project_name(&cwd))))
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let matches = sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.available && s.metadata.cwd == cwd && s.metadata.name == name)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if matches.len() == 1 && sessions[matches[0]].bindings.is_empty() {
            sessions[matches[0]].probable.push(pane.clone());
        }
    }
}
fn project_source(cwd: &Path) -> Option<PathBuf> {
    use std::io::Read;
    let file = fs::File::open(cwd.join(".pi/settings.json")).ok()?;
    let mut text = String::new();
    file.take(1024 * 1024).read_to_string(&mut text).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = paths::expand_home(Path::new(settings["sessionDir"].as_str()?));
    let path = paths::canonical(&if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    });
    path.is_dir().then_some(path)
}

#[derive(Debug, Clone)]
pub struct ResumeSpec {
    pub file: PathBuf,
    pub cwd: String,
    pub title: String,
    pub native_id: String,
    pub warning: String,
}
impl ResumeSpec {
    pub fn launch(&self) -> Result<()> {
        ensure!(
            pi::header(&self.file)?.native_id == self.native_id,
            "Pi identity changed before launch"
        );
        ensure!(
            Path::new(&self.cwd).is_dir(),
            "project directory is unavailable"
        );
        crate::tmux::resume(&self.file, Path::new(&self.cwd), &self.title)
    }
    #[cfg(test)]
    fn launch_program(&self, program: &std::ffi::OsStr) -> Result<()> {
        let status = std::process::Command::new(program)
            .arg("--session")
            .arg(&self.file)
            .current_dir(&self.cwd)
            .status()?;
        ensure!(status.success(), "Pi exited with an error");
        Ok(())
    }
}
fn default_source() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        return absolute(PathBuf::from(dir));
    }
    let agent = paths::pi_dir();
    for settings in [
        std::env::current_dir()?.join(".pi/settings.json"),
        agent.join("settings.json"),
    ] {
        if let Ok(file) = fs::File::open(settings) {
            use std::io::Read;
            let mut data = String::new();
            file.take(1024 * 1024).read_to_string(&mut data)?;
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(dir) = value["sessionDir"].as_str()
            {
                return absolute(PathBuf::from(dir));
            }
        }
    }
    absolute(agent.join("sessions"))
}
fn absolute(path: PathBuf) -> Result<PathBuf> {
    let path = paths::expand_home(&path);
    Ok(paths::canonical(&if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pane(title: &str) -> tmux::PaneIdentity {
        tmux::PaneIdentity {
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
            title: title.into(),
            provider: Some(Provider::Pi),
        }
    }
    fn session() -> Session {
        Session {
            id: "one".into(),
            metadata: Metadata {
                name: "release-notes".into(),
                cwd: "/repo/project".into(),
                ..Default::default()
            },
            state: ResumeState::History,
            note: "retained".into(),
            done_revision: 0,
            done_at: 0,
            available: true,
            presence_verified: false,
            bindings: vec![],
            probable: vec![],
        }
    }
    #[test]
    fn passive_titles_are_hints_and_duplicates_are_never_guessed() {
        let pane = pane("π - release-notes - project");
        let mut sessions = vec![session()];
        associate_probable(&mut sessions, std::slice::from_ref(&pane));
        assert_eq!(sessions[0].probable.len(), 1);
        assert!(sessions[0].bindings.is_empty());
        assert_eq!(sessions[0].state, ResumeState::History);
        assert_eq!(sessions[0].note, "retained");
        assert_eq!(sessions[0].presence(), "Probable opening");
        assert!(View::Open.matches(&sessions[0]));
        sessions.push(session());
        associate_probable(&mut sessions, std::slice::from_ref(&pane));
        assert!(sessions.iter().all(|s| s.probable.is_empty()));
        sessions.pop();
        let mut other = pane.clone();
        other.cwd = "/repo/other".into();
        associate_probable(&mut sessions, &[other]);
        assert!(sessions[0].probable.is_empty());
        associate_probable(&mut sessions, &[pane.clone(), pane]);
        assert_eq!(sessions[0].probable.len(), 2);
        associate_probable(&mut sessions, &[]);
        assert!(sessions[0].probable.is_empty());
    }
    #[test]
    fn project_settings_find_custom_storage_without_an_extension() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".pi")).unwrap();
        fs::create_dir(dir.path().join("history")).unwrap();
        fs::write(
            dir.path().join(".pi/settings.json"),
            r#"{"sessionDir":"history"}"#,
        )
        .unwrap();
        assert_eq!(
            project_source(dir.path()).unwrap(),
            paths::canonical(&dir.path().join("history"))
        );
    }
    #[test]
    fn changed_native_identity_is_rejected_before_any_tmux_creation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.jsonl");
        std::fs::write(
            &file,
            r#"{"type":"session","version":3,"id":"replacement","cwd":"/tmp"}
"#,
        )
        .unwrap();
        let spec = ResumeSpec {
            file,
            cwd: dir.path().to_string_lossy().into_owned(),
            title: "original".into(),
            native_id: "original".into(),
            warning: String::new(),
        };
        assert!(
            spec.launch()
                .unwrap_err()
                .to_string()
                .contains("Pi identity changed")
        );
    }
    #[test]
    fn launch_uses_exact_argv_and_cwd_not_a_shell_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project with spaces");
        fs::create_dir(&cwd).unwrap();
        let script = dir.path().join("fake-pi");
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > \"$PWD/result\"\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let file = cwd.join("session; name.jsonl");
        let spec = ResumeSpec {
            file: file.clone(),
            cwd: cwd.to_string_lossy().into(),
            title: "Synthetic session".into(),
            native_id: String::new(),
            warning: String::new(),
        };
        spec.launch_program(script.as_os_str()).unwrap();
        let output = fs::read_to_string(cwd.join("result")).unwrap();
        let lines = output.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], paths::canonical(&cwd).to_string_lossy());
        assert_eq!(lines[1], "--session");
        assert_eq!(lines[2], file.to_string_lossy());
        assert_eq!(lines.len(), 3);
    }
}
