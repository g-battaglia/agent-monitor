//! TUI event loop: input mutates local state or queues worker operations.
//!
//! No database, tmux, or file reads happen here—only navigation state,
//! search strings, dialog state, and messages to the background worker.
//! Returns `true` from `key()` only when the monitor itself should quit;
//! Pi launches happen outside input handling, after terminal restore.
use super::{
    App, Panel, render,
    worker::{Action, DetailQuery, Reply, Worker},
};
use crate::{model::*, paths};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
};
use std::time::Duration;

/// One modal at a time. Pickers carry their list plus cursor row;
/// confirmations carry the frozen ids they act on, so a changed
/// selection can never retarget a queued mutation.
enum Dialog {
    Help(super::menu::CommandMenu),
    Note {
        id: String,
        text: String,
    },
    Views(usize),
    Projects(usize),
    Openings(Vec<Binding>, usize),
    Probable(Vec<crate::tmux::PaneIdentity>, usize, String),
    Branches(String, Vec<(String, String)>, usize),
    Confirm {
        id: String,
        name: String,
        cwd: String,
        warning: String,
    },
    Panes(usize),
    Info(String),
}
/// The running TUI: local app state, one dialog, the worker handle,
/// and detail-view bookkeeping (branch, tool filter, page, version).
struct Runtime {
    worker: Worker,
    app: App,
    dialog: Option<Dialog>,
    detail_generation: u64,
    detail_version: Option<(String, u64, Option<String>, bool, usize)>,
    branch: Option<String>,
    tools: bool,
    page: usize,
    pending_g: bool,
    touched: bool,
}
impl Runtime {
    /// Queue one worker operation. Writes are never applied locally;
    /// the status line only says "saved" on the worker's own ack.
    fn action(&mut self, action: Action) {
        let writing = matches!(action, Action::Change { .. } | Action::Undo(_));
        if self.worker.actions.try_send(action).is_err() {
            self.app.status = "Operation not sent: queue is busy, retry".into();
        } else if writing {
            self.app.status = "Saving…".into();
        }
    }
    /// Request the detail page for the current selection, if stale.
    /// Latest-wins: only the newest generation is ever rendered, so fast
    /// scrolling never shows an older conversation under a newer row.
    fn detail(&mut self) {
        if self
            .detail_version
            .as_ref()
            .is_some_and(|old| self.app.selected.as_deref() != Some(old.0.as_str()))
        {
            self.branch = None;
            self.page = 0;
        }
        self.app.branch_selected = self.branch.is_some();
        let Some(session) = self.app.current() else {
            return;
        };
        if !session.available {
            let id = session.id.clone();
            let warning = if session.metadata.warning.is_empty() {
                "Conversation unavailable".into()
            } else {
                session.metadata.warning.clone()
            };
            self.app.detail_id = Some(id);
            self.app.conversation = Conversation {
                warning,
                ..Default::default()
            };
            return;
        }
        let leaf = self
            .branch
            .clone()
            .or_else(|| session.bindings.first().and_then(|b| b.bridge.leaf.clone()));
        let version = (
            session.id.clone(),
            session.metadata.revision,
            leaf.clone(),
            self.tools,
            self.page,
        );
        if self.detail_version.as_ref() == Some(&version) {
            return;
        }
        let branch_changed = self.detail_version.as_ref().is_some_and(|old| {
            old.0 == version.0 && (old.2 != version.2 || old.3 != version.3 || old.4 != version.4)
        });
        self.detail_generation += 1;
        self.worker.query(DetailQuery {
            id: session.id.clone(),
            generation: self.detail_generation,
            leaf,
            tools: self.tools,
            page: self.page,
        });
        if branch_changed {
            self.app.detail_id = None;
        }
        self.detail_version = Some(version);
    }
    fn filter_changed(&mut self) {
        self.app.refresh_filter();
        self.app.select(0);
        self.branch = None;
        self.page = 0;
        self.action(Action::Preferences {
            project: self.app.project.clone().unwrap_or_default(),
            view: self.app.view,
        });
    }
    fn step(&mut self, delta: isize) {
        match self.app.panel {
            Panel::Detail => {
                self.app.scroll = (self.app.scroll.min(self.app.max_scroll.get()) as isize + delta)
                    .clamp(0, self.app.max_scroll.get() as isize)
                    as u16
            }
            Panel::Projects => {
                self.app.project_row = (self.app.project_row as isize + delta)
                    .clamp(0, self.app.projects().len() as isize)
                    as usize
            }
            Panel::Sessions => {
                self.app
                    .select((self.app.row as isize + delta).max(0) as usize);
                self.branch = None;
                self.page = 0;
            }
        }
    }
    fn key(&mut self, key: KeyEvent, height: u16) -> bool {
        self.touched = true;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        if self.dialog.is_some() {
            self.dialog_key(key);
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('f') {
            self.app.editing_search = true;
            return false;
        }
        if self.app.editing_search {
            let search = self.app.search_text_mut();
            match key.code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.app.editing_search = false;
                    if key.code == KeyCode::Enter && self.app.panel == Panel::Detail {
                        self.app.scroll =
                            self.app.search_match.get().min(self.app.max_scroll.get());
                    }
                }
                KeyCode::Backspace => {
                    search.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    search.clear()
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && search.chars().count() < 256 =>
                {
                    search.push(c)
                }
                _ => {}
            }
            if self.app.panel != Panel::Detail {
                self.app.refresh_filter();
                if self.app.panel == Panel::Projects {
                    self.app.project_row = usize::from(
                        !self.app.project_search.is_empty() && !self.app.projects().is_empty(),
                    );
                    if key.code == KeyCode::Enter && self.app.project_row > 0 {
                        self.app.project = self.app.projects().first().cloned();
                        self.app.panel = Panel::Sessions;
                        self.filter_changed();
                    }
                } else {
                    self.app.select(0);
                }
            }
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.step((height / 2).max(1) as isize),
                KeyCode::Char('u') => self.step(-((height / 2).max(1) as isize)),
                KeyCode::Char('c') => return true,
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('j') | KeyCode::Down => self.step(1),
            KeyCode::Char('k') | KeyCode::Up => self.step(-1),
            KeyCode::Char('h') | KeyCode::Left => {
                self.app.panel = match self.app.panel {
                    Panel::Detail => Panel::Sessions,
                    _ => Panel::Projects,
                };
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.app.panel = match self.app.panel {
                    Panel::Projects => Panel::Sessions,
                    _ => Panel::Detail,
                };
            }
            KeyCode::BackTab => {
                self.app.panel = match self.app.panel {
                    Panel::Detail => Panel::Sessions,
                    Panel::Sessions => Panel::Projects,
                    Panel::Projects => Panel::Detail,
                }
            }
            KeyCode::Tab => {
                self.app.panel = match self.app.panel {
                    Panel::Projects => Panel::Sessions,
                    Panel::Sessions => Panel::Detail,
                    Panel::Detail => Panel::Projects,
                }
            }
            KeyCode::Char('1') => self.app.panel = Panel::Projects,
            KeyCode::Char('2') => self.app.panel = Panel::Sessions,
            KeyCode::Char('3') => self.app.panel = Panel::Detail,
            KeyCode::Char('g') => {
                if self.pending_g {
                    self.step(-1_000_000);
                }
                self.pending_g = !self.pending_g;
            }
            KeyCode::Char('G') => self.step(1_000_000),
            KeyCode::Char('/') => self.app.editing_search = true,
            KeyCode::Char('z') => {
                self.app.zoom = !self.app.zoom;
                self.app.panel = Panel::Detail;
            }
            KeyCode::Esc => {
                if self.app.zoom {
                    self.app.zoom = false;
                } else if self.app.panel == Panel::Detail {
                    self.app.panel = Panel::Sessions;
                } else {
                    if self.app.panel == Panel::Projects {
                        self.app.project_search.clear();
                    } else {
                        self.app.search.clear();
                    }
                    self.app.detail_search.clear();
                    self.app.refresh_filter();
                    self.app.select(0);
                }
            }
            KeyCode::Char('f') => {
                self.dialog = Some(Dialog::Views(
                    View::ALL
                        .iter()
                        .position(|v| *v == self.app.view)
                        .unwrap_or(0),
                ))
            }
            KeyCode::Char('p') => self.dialog = Some(Dialog::Projects(self.app.project_row)),
            KeyCode::Char('o') => self.dialog = Some(Dialog::Panes(0)),
            KeyCode::Char('i') => {
                if let Some(s) = self.app.current() {
                    self.dialog = Some(Dialog::Info(format!(
                        "Session info\n\nName: {}\nFolder: {}\nFile: {}\nPi ID: {}\nCreated: {}\n\nEsc closes",
                        paths::line(&s.metadata.title()),
                        paths::line(&s.metadata.cwd),
                        paths::line(&s.metadata.file.to_string_lossy()),
                        paths::line(&s.metadata.native_id),
                        paths::date(s.metadata.created)
                    )));
                }
            }
            KeyCode::Char('?') => {
                self.dialog = Some(Dialog::Help(super::menu::CommandMenu::new(&self.app)))
            }
            KeyCode::Char('t') => {
                self.tools = !self.tools;
            }
            KeyCode::Char('[') => {
                self.page = (self.page + 1).min(self.app.conversation.pages.saturating_sub(1));
                self.app.scroll = 0;
            }
            KeyCode::Char(']') => {
                self.page = self.page.saturating_sub(1);
                self.app.scroll = 0;
            }
            KeyCode::Char('b') if self.app.detail_id == self.app.selected => {
                if let Some(id) = &self.app.selected {
                    self.dialog = Some(Dialog::Branches(
                        id.clone(),
                        self.app.conversation.branches.clone(),
                        0,
                    ));
                }
            }
            KeyCode::Char('n') => {
                if let Some(s) = self.app.current() {
                    self.dialog = Some(Dialog::Note {
                        id: s.id.clone(),
                        text: s.note.clone(),
                    });
                }
            }
            KeyCode::Char('d') | KeyCode::Char('r') => {
                if let Some(s) = self.app.current() {
                    self.action(Action::Change {
                        id: s.id.clone(),
                        state: Some(if key.code == KeyCode::Char('d') {
                            ResumeState::Done
                        } else {
                            ResumeState::Resume
                        }),
                        note: None,
                    });
                }
            }
            KeyCode::Char('u') => {
                if let Some(s) = self.app.current() {
                    self.action(Action::Undo(s.id.clone()));
                }
            }
            KeyCode::Char('R') => self.action(Action::Refresh),
            KeyCode::Enter => {
                if self.app.panel == Panel::Projects {
                    self.app.project = self
                        .app
                        .project_row
                        .checked_sub(1)
                        .and_then(|i| self.app.projects().get(i).cloned());
                    self.app.panel = Panel::Sessions;
                    self.filter_changed();
                } else if let Some(s) = self.app.current() {
                    match s.bindings.len() {
                        0 if !s.probable.is_empty() => {
                            self.dialog = Some(Dialog::Probable(
                                s.probable.clone(),
                                0,
                                format!(
                                    "{} / {}",
                                    project_label(&s.metadata.cwd),
                                    s.metadata.title()
                                ),
                            ))
                        }
                        0 => self.action(Action::Prepare(s.id.clone())),
                        1 => self.action(Action::Focus(Box::new(s.bindings[0].clone()))),
                        _ => self.dialog = Some(Dialog::Openings(s.bindings.clone(), 0)),
                    }
                }
            }
            _ => {}
        }
        if key.code != KeyCode::Char('g') {
            self.pending_g = false;
        }
        false
    }
    fn dialog_key(&mut self, key: KeyEvent) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        if key.code == KeyCode::Esc {
            return;
        }
        let delta = match key.code {
            KeyCode::Char('j') | KeyCode::Down => 1,
            KeyCode::Char('k') | KeyCode::Up => -1,
            _ => 0,
        };
        let accepted = key.code == KeyCode::Enter;
        let keep = match &mut dialog {
            Dialog::Info(_) => !accepted,
            Dialog::Help(menu) => {
                if matches!(key.code, KeyCode::Char('?') | KeyCode::Char('q')) {
                    false
                } else {
                    menu.selected = (menu.selected as isize + delta)
                        .clamp(0, menu.actions.len().saturating_sub(1) as isize)
                        as usize;
                    let command = if accepted {
                        menu.actions.get(menu.selected)
                    } else {
                        menu.actions.iter().find(|a| {
                            a.key == key.code
                                && !matches!(key.code, KeyCode::Char('j') | KeyCode::Char('k'))
                        })
                    };
                    if let Some(command) = command {
                        if !command.enabled {
                            self.app.status = "Action unavailable in this context".into();
                            true
                        } else if self.app.panel != Panel::Projects
                            && menu.target != self.app.selected
                        {
                            self.app.status = "Selection changed: reopen the commands".into();
                            false
                        } else {
                            let code = command.key;
                            self.app.modal = None;
                            self.app.command_menu = None;
                            self.key(KeyEvent::new(code, KeyModifiers::NONE), 24);
                            false
                        }
                    } else {
                        true
                    }
                }
            }
            Dialog::Note { id, text } => {
                if accepted {
                    self.action(Action::Change {
                        id: id.clone(),
                        state: None,
                        note: Some(text.clone()),
                    });
                    false
                } else {
                    match key.code {
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Char(c) if text.chars().count() < 4000 => text.push(c),
                        _ => {}
                    }
                    true
                }
            }
            Dialog::Views(row) => {
                *row = (*row as isize + delta).clamp(0, 3) as usize;
                if accepted {
                    self.app.view = View::ALL[*row];
                    self.filter_changed();
                    false
                } else {
                    true
                }
            }
            Dialog::Projects(row) => {
                *row =
                    (*row as isize + delta).clamp(0, self.app.projects().len() as isize) as usize;
                if accepted {
                    self.app.project_row = *row;
                    self.app.project = row
                        .checked_sub(1)
                        .and_then(|i| self.app.projects().get(i).cloned());
                    self.filter_changed();
                    false
                } else {
                    true
                }
            }
            Dialog::Openings(bindings, row) => {
                *row = (*row as isize + delta).clamp(0, bindings.len().saturating_sub(1) as isize)
                    as usize;
                if accepted {
                    if let Some(b) = bindings.get(*row) {
                        self.action(Action::Focus(Box::new(b.clone())));
                    }
                    false
                } else {
                    true
                }
            }
            Dialog::Probable(panes, row, _) => {
                *row = (*row as isize + delta).clamp(0, panes.len().saturating_sub(1) as isize)
                    as usize;
                if accepted {
                    if let Some(pane) = panes.get(*row) {
                        self.action(Action::FocusPane(Box::new(pane.clone())));
                    }
                    false
                } else {
                    true
                }
            }
            Dialog::Branches(id, branches, row) => {
                *row = (*row as isize + delta).clamp(0, branches.len().saturating_sub(1) as isize)
                    as usize;
                if accepted && self.app.selected.as_ref() != Some(id) {
                    self.app.status = "Selection changed: reopen the branch picker".into();
                    false
                } else if accepted {
                    self.branch = branches.get(*row).map(|b| b.0.clone());
                    self.page = 0;
                    self.app.scroll = 0;
                    false
                } else {
                    true
                }
            }
            Dialog::Confirm { id, warning, .. } => {
                if matches!(
                    key.code,
                    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('s')
                ) {
                    self.action(Action::Resume {
                        id: id.clone(),
                        allow_uncertain: !warning.is_empty(),
                    });
                    false
                } else {
                    key.code != KeyCode::Char('n')
                }
            }
            Dialog::Panes(row) => {
                *row = (*row as isize + delta)
                    .clamp(0, self.app.catalog.unbound.len().saturating_sub(1) as isize)
                    as usize;
                if accepted {
                    if let Some(pane) = self.app.catalog.unbound.get(*row) {
                        self.dialog = Some(Dialog::Probable(
                            vec![pane.clone()],
                            0,
                            project_label(&pane.cwd),
                        ));
                    }
                    false
                } else {
                    true
                }
            }
        };
        if keep {
            self.dialog = Some(dialog);
        }
    }
    fn paste(&mut self, text: String) {
        if let Some(Dialog::Note { text: note, .. }) = &mut self.dialog {
            let clipped = text
                .chars()
                .take(4000 - note.chars().count().min(4000))
                .collect::<String>();
            note.push_str(&paths::clean(&clipped));
        } else if self.app.editing_search {
            let search = self.app.search_text_mut();
            let clipped = text
                .chars()
                .take(256 - search.chars().count().min(256))
                .collect::<String>();
            search.push_str(&paths::line(&clipped));
            if self.app.panel != Panel::Detail {
                self.app.refresh_filter();
                if self.app.panel == Panel::Projects {
                    self.app.project_row = usize::from(!self.app.projects().is_empty());
                } else {
                    self.app.select(0);
                }
                self.app.scroll = 0;
            }
        }
        self.modal();
    }
    /// Render the active dialog to text. The `?` menu is special: it keeps
    /// a structured `CommandMenu` for clickable rows instead of flat text.
    fn modal(&mut self) {
        let text = self.dialog.as_ref().map(|dialog| match dialog {
            Dialog::Help(_) => String::new(),
            Dialog::Info(text) => text.clone(),
            Dialog::Note { text, .. } => {
                format!("Next step\n\n{text}▏\n\nEnter saves · Esc cancels")
            }
            Dialog::Views(row) => picker(
                "Show",
                View::ALL.iter().map(|v| v.label().to_string()).collect(),
                *row,
            ),
            Dialog::Projects(row) => picker(
                "Projects",
                std::iter::once("All projects".into())
                    .chain(self.app.projects().iter().map(|c| paths::line(c)))
                    .collect(),
                *row,
            ),
            Dialog::Openings(bindings, row) => picker(
                "Session openings",
                bindings
                    .iter()
                    .map(|b| {
                        b.pane.as_ref().map(|p| format!("{}  {}", p.target, p.title)).unwrap_or_else(|| "Terminal outside tmux".into())
                    })
                    .collect(),
                *row,
            ),
            Dialog::Probable(panes, row, context) => format!(
                "{}\n\n{}\n\nName and folder match. Pi identity is unconfirmed.\nEnter opens this window · Esc cancels",
                paths::line(context),
                picker(
                    "Where do you want to continue?",
                    panes.iter().enumerate().map(|(i, p)| format!("Opening {} — window {} of {}", i + 1, paths::line(&p.window), project_label(&p.cwd))).collect(),
                    *row,
                ),
            ),
            Dialog::Branches(_, branches, row) => picker(
                "Conversation branches",
                branches.iter().enumerate().map(|(i, (_, name))| format!("Branch {}  {}", i + 1, paths::line(name))).collect(),
                *row,
            ),
            Dialog::Confirm { name, cwd, warning, .. } => format!(
                "{} / {}\n\nReopen the same Pi conversation?\n\n{}\n\nNew tmux window in the project, or a new session when the previous one is gone.\n\nEnter reopens · n/Esc cancels",
                project_label(cwd),
                paths::line(name),
                paths::line(warning),
            ),
            Dialog::Panes(row) => format!(
                "{}\n\nEnter explores the pane · Esc closes.\nThe extension is optional and verifies the exact file.",
                picker(
                    "Pi panes without a conversation identity",
                    self.app.catalog.unbound.iter().map(|p| format!("{}  {}", paths::line(&p.target), paths::line(&p.title))).collect(),
                    *row,
                ),
            ),
        });
        self.app.command_menu = match &self.dialog {
            Some(Dialog::Help(menu)) => Some(menu.clone()),
            _ => None,
        };
        self.app.modal = text.filter(|text| !text.is_empty());
    }
}
fn picker(title: &str, items: Vec<String>, row: usize) -> String {
    let start = row.saturating_sub(7);
    format!(
        "{title}\n\n{}\n\n↑/↓ choose · Enter confirms · Esc cancels",
        items
            .iter()
            .enumerate()
            .skip(start)
            .take(8)
            .map(|(i, item)| format!("{} {}", if i == row { "›" } else { " " }, paths::line(item)))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub fn run(root: std::path::PathBuf, project: Option<String>) -> Result<()> {
    use std::io::IsTerminal;
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "Run the monitor in an interactive terminal; for scripts use sessions --json"
    );
    let mut runtime = Runtime {
        worker: Worker::start(root),
        app: App::default(),
        dialog: None,
        detail_generation: 0,
        detail_version: None,
        branch: None,
        tools: false,
        page: 0,
        pending_g: false,
        touched: false,
    };
    runtime.app.project = project;
    runtime.app.catalog.scanning = true;
    let mut terminal = ratatui::init();
    let mut first_catalog = true;
    let mut dirty = true;
    let result = (|| -> Result<()> {
        execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        loop {
            while let Ok(reply) = runtime.worker.replies.try_recv() {
                dirty = true;
                match reply {
                    Reply::Preferences { project, view } => {
                        if !runtime.touched {
                            if runtime.app.project.is_none() {
                                runtime.app.project = project;
                            }
                            runtime.app.view = view;
                        }
                    }
                    Reply::Catalog(catalog) => {
                        if let Some(project) = runtime.app.project.clone() {
                            let paths = catalog
                                .sessions
                                .iter()
                                .map(|s| s.metadata.cwd.clone())
                                .chain(catalog.unbound.iter().map(|p| p.cwd.clone()))
                                .collect::<std::collections::BTreeSet<_>>();
                            let matches = paths
                                .into_iter()
                                .filter(|p| {
                                    p == &project
                                        || crate::model::project_name(p) == project
                                        || project_label(p).eq_ignore_ascii_case(&project)
                                })
                                .collect::<Vec<_>>();
                            if matches.len() == 1 {
                                runtime.app.project = Some(matches[0].clone());
                            }
                        }
                        if first_catalog && !catalog.sessions.is_empty() {
                            if runtime.app.project.is_none() && !runtime.touched {
                                let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
                                if catalog.sessions.iter().any(|s| s.metadata.cwd == cwd) {
                                    runtime.app.project = Some(cwd);
                                }
                            }
                            first_catalog = false;
                        }
                        runtime.app.apply(catalog);
                    }
                    Reply::Detail { query, result } => {
                        if query.generation == runtime.detail_generation
                            && runtime.app.selected.as_deref() == Some(&query.id)
                        {
                            match result {
                                Ok(c) => {
                                    runtime.app.conversation = c;
                                    runtime.app.detail_id = Some(query.id);
                                }
                                Err(e) => {
                                    runtime.app.status = e.clone();
                                    runtime.app.conversation = Conversation {
                                        warning: e,
                                        ..Default::default()
                                    };
                                    runtime.app.detail_id = Some(query.id);
                                }
                            }
                        }
                    }
                    Reply::Ack(result) => match result {
                        Ok(s) => {
                            if !s.is_empty() {
                                runtime.app.status = s;
                            }
                        }
                        Err(s) => runtime.app.status = s,
                    },
                    Reply::Confirm { id, spec } => {
                        let name = spec.title;
                        runtime.dialog = Some(Dialog::Confirm {
                            id,
                            name,
                            cwd: spec.cwd,
                            warning: spec.warning,
                        });
                    }
                    Reply::Launch(spec) => {
                        // Terminal ownership passes to Pi only after explicit
                        // confirmation plus worker revalidation, outside input.
                        execute!(
                            std::io::stdout(),
                            DisableMouseCapture,
                            DisableBracketedPaste
                        )?;
                        ratatui::restore();
                        let result = spec.launch();
                        if result.is_ok() {
                            runtime.action(Action::Managed(spec.file.clone()));
                        }
                        terminal = ratatui::init();
                        execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
                        runtime.app.status = result
                            .err()
                            .map(|e| paths::line(&format!("{e:#}")))
                            .unwrap_or_else(|| "Tornato al monitor".into());
                        runtime.detail_version = None;
                        runtime.action(Action::Refresh);
                    }
                }
            }
            runtime.detail();
            runtime.modal();
            if dirty {
                terminal.draw(|frame| render::draw(frame, &runtime.app))?;
                dirty = false;
            }
            if event::poll(Duration::from_millis(20))? {
                dirty = true;
                match event::read()? {
                    Event::Paste(text) => runtime.paste(text),
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if runtime.key(key, terminal.size()?.height) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) if matches!(runtime.dialog, Some(Dialog::Help(_))) => {
                        if let Some(Dialog::Help(menu)) = &mut runtime.dialog {
                            let rows = menu.rows();
                            let rect =
                                render::menu_list_area(terminal.get_frame().area(), rows.len());
                            match mouse.kind {
                                MouseEventKind::ScrollDown => {
                                    menu.selected = (menu.selected + 1).min(menu.actions.len() - 1)
                                }
                                MouseEventKind::ScrollUp => {
                                    menu.selected = menu.selected.saturating_sub(1)
                                }
                                MouseEventKind::Down(event::MouseButton::Left)
                                    if rect.contains(ratatui::layout::Position::new(
                                        mouse.column,
                                        mouse.row,
                                    )) =>
                                {
                                    let index = runtime.app.menu_offset.get()
                                        + (mouse.row - rect.y) as usize;
                                    if let Some(Some(selected)) = rows.get(index) {
                                        menu.selected = *selected;
                                        runtime.key(
                                            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                                            24,
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Event::Mouse(mouse) if runtime.dialog.is_none() => {
                        if matches!(mouse.kind, MouseEventKind::Down(event::MouseButton::Left))
                            && render::search_area(terminal.get_frame().area())
                                .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                        {
                            runtime.app.editing_search = true;
                            continue;
                        }
                        let panes = render::areas(
                            ratatui::layout::Rect::new(
                                0,
                                0,
                                terminal.size()?.width,
                                terminal.size()?.height,
                            ),
                            runtime.app.zoom,
                            runtime.app.panel,
                        );
                        for (i, rect) in panes.into_iter().enumerate() {
                            if rect
                                .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                            {
                                runtime.app.panel =
                                    [Panel::Projects, Panel::Sessions, Panel::Detail][i];
                                match mouse.kind {
                                    MouseEventKind::ScrollDown => runtime.step(3),
                                    MouseEventKind::ScrollUp => runtime.step(-3),
                                    MouseEventKind::Down(event::MouseButton::Left) => {
                                        let row = mouse.row.saturating_sub(rect.y + 1) as usize;
                                        if i == 0 {
                                            runtime.app.project_row =
                                                (runtime.app.project_offset.get() + row)
                                                    .min(runtime.app.projects().len());
                                            runtime.key(
                                                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                                                rect.height,
                                            );
                                        }
                                        if i == 1 {
                                            let offset = runtime.app.session_offset.get();
                                            runtime.app.select(offset + row);
                                            runtime.branch = None;
                                            runtime.page = 0;
                                        }
                                    }
                                    _ => {}
                                }
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Runtime {
        Runtime {
            worker: Worker::idle(),
            app: App::default(),
            dialog: None,
            detail_generation: 0,
            detail_version: None,
            branch: None,
            tools: false,
            page: 0,
            pending_g: false,
            touched: false,
        }
    }
    fn session(id: &str) -> Session {
        Session {
            id: id.into(),
            metadata: Metadata {
                name: id.into(),
                ..Default::default()
            },
            state: ResumeState::Resume,
            note: String::new(),
            done_revision: 0,
            done_at: 0,
            available: true,
            presence_verified: false,
            bindings: vec![],
            probable: vec![],
        }
    }
    #[test]
    fn action_menu_navigates_activates_and_cannot_retarget_mutations() {
        let mut runtime = fixture();
        let (actions, requests) = std::sync::mpsc::sync_channel(16);
        runtime.worker.actions = actions;
        runtime.app.apply(Catalog {
            sessions: vec![session("one"), session("two")],
            ..Default::default()
        });
        runtime.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE), 24);
        assert!(matches!(runtime.dialog, Some(Dialog::Help(_))));
        runtime.modal();
        let row = runtime
            .app
            .command_menu
            .as_ref()
            .unwrap()
            .actions
            .iter()
            .position(|a| a.key == KeyCode::Char('d'))
            .unwrap();
        for _ in 0..row {
            runtime.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), 24);
        }
        runtime.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 24);
        assert!(
            matches!(requests.try_recv().unwrap(),Action::Change{id,state:Some(ResumeState::Done),..} if id=="one")
        );
        assert!(runtime.dialog.is_none());
        runtime.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE), 24);
        runtime.app.select(1);
        runtime.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE), 24);
        assert!(requests.try_recv().is_err());
        assert!(runtime.app.status.contains("Selection changed"));
        runtime.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE), 24);
        runtime.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 24);
        runtime.modal();
        assert!(runtime.app.command_menu.is_none());
    }
    #[test]
    fn note_target_does_not_follow_selection_and_paste_is_not_a_command() {
        let mut runtime = fixture();
        let (actions, requests) = std::sync::mpsc::sync_channel(16);
        runtime.worker.actions = actions;
        runtime.app.apply(Catalog {
            sessions: vec![session("one"), session("two")],
            ..Default::default()
        });
        runtime.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), 24);
        runtime.paste("x".repeat(10_000));
        runtime.app.select(1);
        runtime.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 24);
        let Action::Change { id, note, .. } = requests.try_recv().unwrap() else {
            panic!("expected note");
        };
        assert_eq!(id, "one");
        assert_eq!(note.unwrap().chars().count(), 4000);
        runtime.paste("dddq".into());
        assert!(requests.try_recv().is_err());
        assert_eq!(runtime.app.current().unwrap().state, ResumeState::Resume);
        runtime.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), 24);
        assert!(runtime.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), 24));
    }
    #[test]
    fn new_selection_resets_old_branch_and_rejects_stale_branch_picker() {
        let mut runtime = fixture();
        runtime.app.apply(Catalog {
            sessions: vec![session("one"), session("two")],
            ..Default::default()
        });
        runtime.detail_version = Some(("one".into(), 0, None, false, 0));
        runtime.branch = Some("old-branch".into());
        runtime.page = 3;
        runtime.app.select(1);
        runtime.detail();
        assert!(runtime.branch.is_none());
        assert_eq!(runtime.page, 0);
        runtime.dialog = Some(Dialog::Branches(
            "one".into(),
            vec![("old-branch".into(), "request".into())],
            0,
        ));
        runtime.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 24);
        assert!(runtime.branch.is_none());
        assert!(runtime.app.status.contains("Selection changed"));
    }
    #[test]
    fn visible_search_filters_projects_and_enter_opens_the_result() {
        let mut runtime = fixture();
        let mut one = session("one");
        one.metadata.cwd = "/repo/acme-website".into();
        let mut two = session("two");
        two.metadata.cwd = "/repo/elsewhere".into();
        runtime.app.apply(Catalog {
            sessions: vec![one, two],
            ..Default::default()
        });
        assert_eq!(runtime.app.view, View::All);
        runtime.app.panel = Panel::Projects;
        runtime.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL), 24);
        runtime.paste("acme".into());
        assert_eq!(runtime.app.projects(), &["/repo/acme-website"]);
        assert_eq!(runtime.app.project_row, 1);
        runtime.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 24);
        assert_eq!(runtime.app.project.as_deref(), Some("/repo/acme-website"));
        assert_eq!(runtime.app.panel, Panel::Sessions);
        assert_eq!(runtime.app.current().unwrap().id, "one");
        runtime.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE), 24);
        runtime.paste("one".into());
        assert_eq!(runtime.app.search, "one");
        assert_eq!(runtime.app.project_search, "acme");
    }
    #[test]
    fn polling_keeps_selection_and_viewport() {
        let mut app = App::default();
        app.apply(Catalog {
            sessions: vec![session("one"), session("two")],
            ..Default::default()
        });
        app.select(1);
        app.scroll = 7;
        app.apply(Catalog {
            sessions: vec![session("two"), session("one")],
            ..Default::default()
        });
        assert_eq!(app.current().unwrap().id, "two");
        assert_eq!(app.scroll, 7);
    }
    #[test]
    fn quit_is_local_while_worker_is_blocked() {
        let mut runtime = fixture();
        let start = std::time::Instant::now();
        assert!(runtime.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), 24));
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
