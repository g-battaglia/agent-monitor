//! The contextual action menu behind `?`.
use super::{App, Panel};
use crate::model::{ResumeState, project_label};
use crossterm::event::KeyCode;

#[derive(Clone)]
pub struct MenuAction {
    pub group: &'static str,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub key: KeyCode,
    pub description: &'static str,
    pub enabled: bool,
}
#[derive(Clone)]
pub struct CommandMenu {
    pub title: &'static str,
    pub context: String,
    pub target: Option<String>,
    pub actions: Vec<MenuAction>,
    pub selected: usize,
}
impl CommandMenu {
    pub fn new(app: &App) -> Self {
        let mut menu = Self {
            title: match app.panel {
                Panel::Projects => "Project commands",
                Panel::Sessions => "Session commands",
                Panel::Detail => "Conversation commands",
            },
            context: app
                .current()
                .map(|s| {
                    format!(
                        "{} / {}",
                        project_label(&s.metadata.cwd),
                        s.metadata.title()
                    )
                })
                .unwrap_or_else(|| "Choose what to do".into()),
            target: app.selected.clone(),
            actions: vec![],
            selected: 0,
        };
        menu.add(
            "Trova",
            match app.panel {
                Panel::Projects => "Search projects",
                Panel::Sessions => "Search sessions",
                Panel::Detail => "Search the text",
            },
            "/",
            KeyCode::Char('/'),
            "Type in the search bar at the top.",
            true,
        );
        if app.panel == Panel::Projects {
            menu.context = app
                .project_row
                .checked_sub(1)
                .and_then(|i| app.projects().get(i))
                .map(|p| project_label(p))
                .unwrap_or_else(|| "All projects".into());
            menu.add(
                "Project",
                "Open the selected project",
                "Enter",
                KeyCode::Enter,
                "Show this project's conversations.",
                true,
            );
        } else {
            let session = app.current();
            let persistent = session.is_some_and(|s| !s.id.starts_with("live-"));
            menu.add(
                "Session",
                "Go to Pi or resume the work",
                "Enter",
                KeyCode::Enter,
                "When the pane is gone, reopen the same conversation in tmux.",
                session.is_some(),
            );
            menu.add(
                "Session",
                "Edit the next step",
                "n",
                KeyCode::Char('n'),
                "Save a note for when you return to this conversation.",
                persistent,
            );
            menu.add(
                "Session",
                "Mark as done",
                "d",
                KeyCode::Char('d'),
                "Never closes Pi or tmux: only changes the work state.",
                persistent && session.is_some_and(|s| s.state != ResumeState::Done),
            );
            menu.add(
                "Session",
                "Mark to resume",
                "r",
                KeyCode::Char('r'),
                "Keep this conversation in the to-resume list.",
                persistent && session.is_some_and(|s| s.state != ResumeState::Resume),
            );
            menu.add(
                "Session",
                "Undo the last change",
                "u",
                KeyCode::Char('u'),
                "Restore the previous note or state decision.",
                persistent,
            );
            menu.add(
                "Session",
                "Show file info",
                "i",
                KeyCode::Char('i'),
                "Path and conversation identity, kept apart from display names.",
                session.is_some(),
            );
            if app.panel == Panel::Detail {
                menu.add(
                    "Reading",
                    if app.zoom {
                        "Shrink the conversation"
                    } else {
                        "Expand the conversation"
                    },
                    "z",
                    KeyCode::Char('z'),
                    "Use the whole terminal to read the text.",
                    session.is_some(),
                );
                menu.add(
                    "Reading",
                    "Choose a conversation branch",
                    "b",
                    KeyCode::Char('b'),
                    "Explore alternatives without mixing branch history.",
                    app.detail_id == app.selected && !app.conversation.branches.is_empty(),
                );
                menu.add(
                    "Reading",
                    "Show or hide tools",
                    "t",
                    KeyCode::Char('t'),
                    "Internal reasoning always stays hidden.",
                    session.is_some(),
                );
                menu.add(
                    "Reading",
                    "Read the older page",
                    "[",
                    KeyCode::Char('['),
                    "Move toward older exchanges.",
                    app.conversation.page + 1 < app.conversation.pages,
                );
                menu.add(
                    "Reading",
                    "Read the newer page",
                    "]",
                    KeyCode::Char(']'),
                    "Move back toward recent exchanges.",
                    app.conversation.page > 0,
                );
            }
        }
        menu.add(
            "Navigate",
            "Change view",
            "f",
            KeyCode::Char('f'),
            "All, to resume, open, or done.",
            true,
        );
        menu.add(
            "Navigate",
            "Pick another project",
            "p",
            KeyCode::Char('p'),
            "Projects come automatically from Pi folders.",
            true,
        );
        menu.add(
            "Navigate",
            "Browse open Pi runs",
            "o",
            KeyCode::Char('o'),
            "Pick an existing pane, even without the extension.",
            true,
        );
        menu.add(
            "Navigate",
            "Move to the next panel",
            "Tab",
            KeyCode::Tab,
            "Left/right switch panels; up/down scroll.",
            true,
        );
        menu.add(
            "Catalog",
            "Refresh the catalog",
            "R",
            KeyCode::Char('R'),
            "Re-read names, conversations, and openings without starting agents.",
            true,
        );
        menu
    }
    fn add(
        &mut self,
        group: &'static str,
        label: &'static str,
        shortcut: &'static str,
        key: KeyCode,
        description: &'static str,
        enabled: bool,
    ) {
        self.actions.push(MenuAction {
            group,
            label,
            shortcut,
            key,
            description,
            enabled,
        });
    }
    /// Row layout: `None` is a group heading, `Some(i)` is the i-th action.
    /// Headings are display-only and never selectable.
    pub fn rows(&self) -> Vec<Option<usize>> {
        let mut rows = vec![];
        let mut group = "";
        for (i, action) in self.actions.iter().enumerate() {
            if action.group != group {
                rows.push(None);
                group = action.group;
            }
            rows.push(Some(i));
        }
        rows
    }
}
