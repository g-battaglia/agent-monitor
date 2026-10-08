//! Local navigation state plus rendering. No I/O here.
//!
//! `App` owns panels, search strings, filters, and the visible row cache.
//! Rendering and input only read or mutate this struct; the worker thread
//! owns the database, tmux, and Pi file reads. Three search strings exist
//! (projects, sessions, conversation text) so switching panels never
//! loses what was typed.
pub mod menu;
pub mod render;
mod runtime;
mod worker;
use crate::model::{Catalog, Conversation, Session, View, project_label};
pub use runtime::run;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Projects,
    Sessions,
    Detail,
}

/// Everything the TUI renders and navigates. Plain data plus filter
/// caches; pushing updates in from worker replies keeps selection stable.
pub struct App {
    pub catalog: Catalog,
    pub project: Option<String>,
    pub project_row: usize,
    pub view: View,
    pub selected: Option<String>,
    pub row: usize,
    pub panel: Panel,
    pub search: String,
    pub project_search: String,
    pub editing_search: bool,
    pub detail_search: String,
    pub branch_selected: bool,
    pub conversation: Conversation,
    pub detail_id: Option<String>,
    pub scroll: u16,
    pub zoom: bool,
    pub status: String,
    pub no_color: bool,
    pub modal: Option<String>,
    pub command_menu: Option<menu::CommandMenu>,
    pub menu_offset: std::cell::Cell<usize>,
    pub visible: Vec<usize>,
    pub project_paths: Vec<String>,
    pub max_scroll: std::cell::Cell<u16>,
    pub search_match: std::cell::Cell<u16>,
    pub project_offset: std::cell::Cell<usize>,
    pub session_offset: std::cell::Cell<usize>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            catalog: Catalog::default(),
            project: None,
            project_row: 0,
            view: View::All,
            selected: None,
            row: 0,
            panel: Panel::Sessions,
            search: String::new(),
            project_search: String::new(),
            editing_search: false,
            detail_search: String::new(),
            branch_selected: false,
            conversation: Conversation::default(),
            detail_id: None,
            scroll: 0,
            zoom: false,
            status: String::new(),
            no_color: std::env::var_os("NO_COLOR").is_some(),
            modal: None,
            command_menu: None,
            menu_offset: std::cell::Cell::new(0),
            visible: vec![],
            project_paths: vec![],
            max_scroll: std::cell::Cell::new(0),
            search_match: std::cell::Cell::new(0),
            project_offset: std::cell::Cell::new(0),
            session_offset: std::cell::Cell::new(0),
        }
    }
}
impl App {
    pub fn projects(&self) -> &[String] {
        &self.project_paths
    }
    pub fn search_text(&self) -> &str {
        match self.panel {
            Panel::Projects => &self.project_search,
            Panel::Sessions => &self.search,
            Panel::Detail => &self.detail_search,
        }
    }
    pub fn search_text_mut(&mut self) -> &mut String {
        match self.panel {
            Panel::Projects => &mut self.project_search,
            Panel::Sessions => &mut self.search,
            Panel::Detail => &mut self.detail_search,
        }
    }
    /// The All-projects action is an escape hatch from every list filter.
    pub fn choose_project(&mut self, row: usize) {
        self.project_row = row;
        self.project = row
            .checked_sub(1)
            .and_then(|i| self.project_paths.get(i).cloned());
        if row == 0 {
            self.view = View::All;
            self.search.clear();
            self.project_search.clear();
        }
    }
    pub fn refresh_filter(&mut self) {
        let project_needle = self.project_search.to_lowercase();
        self.project_paths = self
            .catalog
            .sessions
            .iter()
            .map(|s| s.metadata.cwd.clone())
            .chain(self.catalog.unbound.iter().map(|p| p.cwd.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|cwd| {
                cwd.to_lowercase().contains(&project_needle)
                    || project_label(cwd).to_lowercase().contains(&project_needle)
            })
            .collect();
        let needle = self.search.to_lowercase();
        self.visible = self
            .catalog
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                self.view.matches(s)
                    && self.project.as_ref().is_none_or(|p| *p == s.metadata.cwd)
                    && (needle.is_empty()
                        || format!(
                            "{} {} {} {} {}",
                            s.metadata.provider.label(),
                            s.metadata.title(),
                            s.metadata.cwd,
                            project_label(&s.metadata.cwd),
                            s.note
                        )
                        .to_lowercase()
                        .contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();
        if self.view == View::All {
            // Open terminals must not be buried under hundreds of historical rows.
            // Stable sorting retains newest-first history within each group.
            self.visible.sort_by_key(|i| {
                let s = &self.catalog.sessions[*i];
                s.bindings.is_empty() && s.probable.is_empty() && !s.id.starts_with("pane-")
            });
        }
    }
    pub fn rows(&self) -> Vec<&Session> {
        self.visible
            .iter()
            .filter_map(|i| self.catalog.sessions.get(*i))
            .collect()
    }
    pub fn current(&self) -> Option<&Session> {
        self.visible
            .get(self.row)
            .and_then(|i| self.catalog.sessions.get(*i))
    }
    pub fn select(&mut self, row: usize) {
        self.row = row.min(self.visible.len().saturating_sub(1));
        let id = self.current().map(|s| s.id.clone());
        if id != self.selected {
            self.scroll = 0;
        }
        self.selected = id;
    }
    pub fn apply(&mut self, catalog: Catalog) {
        let id = self.selected.clone();
        let project_cursor = self
            .project_row
            .checked_sub(1)
            .and_then(|i| self.project_paths.get(i))
            .cloned();
        self.catalog = catalog;
        self.refresh_filter();
        if self.panel != Panel::Projects {
            self.project_row = self
                .project
                .as_ref()
                .and_then(|p| self.project_paths.iter().position(|c| c == p))
                .map(|i| i + 1)
                .unwrap_or(0);
        } else {
            self.project_row = project_cursor
                .and_then(|p| self.project_paths.iter().position(|cwd| cwd == &p))
                .map(|i| i + 1)
                .unwrap_or(self.project_row.min(self.project_paths.len()));
        }
        let row = id
            .and_then(|id| self.rows().iter().position(|s| s.id == id))
            .unwrap_or(self.row);
        self.select(row);
    }
}
