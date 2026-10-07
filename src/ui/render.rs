//! LazyGit-like fixed panels: selection on the left, context on the right.
//!
//! `areas()` is the single source of truth for layout AND mouse targets:
//! the runtime reuses these exact rectangles for clicks. Panels never
//! overlap; narrow terminals collapse to the active panel instead of
//! squeezing unreadable columns side by side.
use super::{App, Panel};
use crate::{
    model::{project_label, project_name},
    paths,
};
use ratatui::{prelude::*, widgets::*};

#[derive(Clone, Copy)]
struct Theme {
    focus: Color,
    muted: Color,
    selection: Style,
    warning: Color,
}
impl Theme {
    fn new(no_color: bool) -> Self {
        if no_color {
            Self {
                focus: Color::Reset,
                muted: Color::Reset,
                selection: Style::new().reversed().bold(),
                warning: Color::Reset,
            }
        } else {
            Self {
                focus: Color::Cyan,
                muted: Color::DarkGray,
                selection: Style::new().bg(Color::DarkGray).fg(Color::White).bold(),
                warning: Color::Yellow,
            }
        }
    }
    fn block(self, title: String, active: bool) -> Block<'static> {
        Block::bordered()
            .border_type(BorderType::Rounded)
            .title(format!(" {title} "))
            .border_style(Style::new().fg(if active { self.focus } else { self.muted }))
            .title_style(if active {
                Style::new().fg(self.focus).bold()
            } else {
                Style::new()
            })
    }
}

/// Split the terminal into [projects, sessions, detail].
/// Wide terminals show all three; medium ones show sessions plus context;
/// narrow or zoomed terminals show only the active panel.
pub fn areas(area: Rect, zoom: bool, panel: Panel) -> [Rect; 3] {
    let search_height = 3.min(area.height);
    let body = Rect {
        y: area.y + search_height,
        height: area.height.saturating_sub(search_height + 2),
        ..area
    };
    let empty = Rect::default();
    if zoom || area.width < 65 {
        return match panel {
            Panel::Detail => [empty, empty, body],
            Panel::Projects => [body, empty, empty],
            _ => [empty, body, empty],
        };
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(body);
    if area.width < 100 {
        return if panel == Panel::Projects {
            [cols[0], empty, cols[1]]
        } else {
            [empty, cols[0], cols[1]]
        };
    }
    let left = Layout::vertical([
        Constraint::Length(7.min(body.height / 3)),
        Constraint::Min(3),
    ])
    .split(cols[0]);
    [left[0], left[1], cols[1]]
}

pub fn search_area(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, 3.min(area.height))
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let theme = Theme::new(app.no_color);
    let label = match app.panel {
        Panel::Projects => "Search projects",
        Panel::Sessions => "Search sessions",
        Panel::Detail => "Search the page text",
    };
    // Hint text only when idle: the moment editing starts the field clears,
    // so the cursor never sits inside the placeholder (see screenshot).
    let query = if app.search_text().is_empty() && !app.editing_search {
        format!(
            "{}  / or Ctrl-f",
            match app.panel {
                Panel::Projects => "Name or folder…",
                Panel::Sessions => "Name, project, or note…",
                Panel::Detail => "Text to find…",
            }
        )
    } else {
        paths::line(app.search_text())
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{}{}",
            query,
            if app.editing_search { "▏" } else { "" }
        ))
        .block(theme.block(label.into(), app.editing_search)),
        search_area(area),
    );
    let panes = areas(area, app.zoom, app.panel);
    if panes[0].width > 0 {
        projects(frame, app, panes[0], theme);
    }
    if panes[1].width > 0 {
        sessions(frame, app, panes[1], theme);
    }
    if panes[2].width > 0 {
        detail(frame, app, panes[2], theme);
    }
    let hints = match app.panel {
        Panel::Projects => "Enter pick project   Tab sessions   / search   ? actions",
        Panel::Sessions if area.width < 65 => "r to resume  Enter open  ? actions",
        Panel::Sessions => "r to resume   d done   Enter open   f view   / search   ? actions",
        Panel::Detail if area.width < 65 => "r to resume  j/k scroll  ? actions",
        Panel::Detail => "r to resume   j/k scroll   [ ] pages   / search   z expand   ? actions",
    };
    if area.height > 1 {
        frame.render_widget(
            Paragraph::new(hints).style(Style::new().fg(theme.muted)),
            Rect::new(area.x, area.bottom() - 2, area.width, 1),
        );
        let status = if app.editing_search {
            "Enter confirms · Esc ends search".to_owned()
        } else if !app.status.is_empty() {
            paths::line(&app.status)
        } else if app.catalog.scanning {
            "Indexing sessions…".into()
        } else {
            app.catalog
                .warnings
                .first()
                .map(|s| paths::line(s))
                .unwrap_or_default()
        };
        frame.render_widget(
            Paragraph::new(status).style(Style::new().fg(theme.warning)),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
    if let Some(menu) = &app.command_menu {
        command_menu(frame, app, menu, theme);
    } else if let Some(text) = &app.modal {
        let w = area.width.saturating_sub(4).min(76);
        let h = area.height.saturating_sub(2).min(16);
        let rect = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(paths::clean(text))
                .wrap(Wrap { trim: false })
                .block(theme.block("Actions".into(), true)),
            rect,
        );
    }
}

pub fn menu_area(area: Rect, count: usize) -> Rect {
    let w = area.width.saturating_sub(4).min(72);
    let h = area.height.saturating_sub(2).min((count + 7) as u16);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
pub fn menu_list_area(area: Rect, count: usize) -> Rect {
    let outer = menu_area(area, count);
    Rect::new(
        outer.x + 1,
        outer.y + 3,
        outer.width.saturating_sub(2),
        outer.height.saturating_sub(7),
    )
}
fn command_menu(frame: &mut Frame, app: &App, menu: &super::menu::CommandMenu, theme: Theme) {
    let rows = menu.rows();
    let outer = menu_area(frame.area(), rows.len());
    let body = menu_list_area(frame.area(), rows.len());
    frame.render_widget(Clear, outer);
    frame.render_widget(theme.block(menu.title.into(), true), outer);
    frame.render_widget(
        Paragraph::new(paths::line(&menu.context)).style(Style::new().fg(theme.muted)),
        Rect::new(outer.x + 2, outer.y + 1, outer.width.saturating_sub(4), 1),
    );
    let mut group = "";
    let items = rows
        .iter()
        .enumerate()
        .map(|(row, item)| {
            if let Some(i) = item {
                let a = &menu.actions[*i];
                group = a.group;
                ListItem::new(Line::from(vec![
                    Span::raw(format!(
                        "{:<width$}",
                        a.label
                            .chars()
                            .take(body.width.saturating_sub(12) as usize)
                            .collect::<String>(),
                        width = body.width.saturating_sub(12) as usize
                    )),
                    Span::styled(a.shortcut, Style::new().fg(theme.muted)),
                ]))
                .style(if a.enabled {
                    Style::new()
                } else {
                    Style::new().fg(theme.muted)
                })
            } else {
                let heading = rows
                    .iter()
                    .skip(row + 1)
                    .find_map(|i| i.map(|i| menu.actions[i].group))
                    .unwrap_or(group);
                ListItem::new(Line::styled(heading, Style::new().fg(theme.focus).bold()))
            }
        })
        .collect::<Vec<_>>();
    let selected = rows
        .iter()
        .position(|i| *i == Some(menu.selected))
        .unwrap_or(0);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .highlight_style(theme.selection),
        body,
        &mut state,
    );
    app.menu_offset.set(state.offset());
    if outer.height >= 7 {
        let description = menu
            .actions
            .get(menu.selected)
            .map(|a| {
                if a.enabled {
                    a.description
                } else {
                    "Action unavailable for this selection."
                }
            })
            .unwrap_or("");
        frame.render_widget(
            Paragraph::new(description)
                .wrap(Wrap { trim: false })
                .style(Style::new().fg(theme.muted)),
            Rect::new(
                outer.x + 2,
                outer.bottom() - 4,
                outer.width.saturating_sub(4),
                2,
            ),
        );
        frame.render_widget(
            Paragraph::new("↑↓ choose   Enter run   Esc close"),
            Rect::new(
                outer.x + 2,
                outer.bottom() - 2,
                outer.width.saturating_sub(4),
                1,
            ),
        );
    }
}

fn projects(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let projects = app.projects();
    // First row is the virtual "everything" project; the rest are real
    // folders. Counts are catalog totals, never filtered by the current view,
    // so history does not misleadingly read as zero while browsing resume.
    let mut items = vec![ListItem::new(format!(
        "All projects  {}",
        app.catalog.sessions.len()
    ))];
    for cwd in projects {
        let name = project_label(cwd);
        let label = if projects.iter().filter(|c| project_label(c) == name).count() > 1 {
            std::path::Path::new(cwd)
                .parent()
                .map(|p| format!("{name} ({})", project_name(&p.to_string_lossy())))
                .unwrap_or(name)
        } else {
            name
        };
        let count = app
            .catalog
            .sessions
            .iter()
            .filter(|s| s.metadata.cwd == *cwd)
            .count();
        let total = if count > 0 {
            count.to_string()
        } else if app.catalog.unbound.iter().any(|p| p.cwd == *cwd) {
            "Open Pi".into()
        } else if app.catalog.scanning {
            "…".into()
        } else {
            "0".into()
        };
        items.push(ListItem::new(format!("{}  {total}", paths::line(&label))));
    }
    let selected = app
        .project
        .as_ref()
        .and_then(|p| projects.iter().position(|c| c == p))
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut state = ListState::default().with_selected(Some(if app.panel == Panel::Projects {
        app.project_row
    } else {
        selected
    }));
    frame.render_stateful_widget(
        List::new(items)
            .block(theme.block(
                "Projects".into(),
                app.panel == Panel::Projects && !app.editing_search,
            ))
            .highlight_style(theme.selection)
            .highlight_symbol("› "),
        area,
        &mut state,
    );
    app.project_offset.set(state.offset());
}

fn sessions(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let rows = app.rows();
    let title = format!(
        "{}{} · {}",
        app.project
            .as_ref()
            .map(|cwd| format!("{} / ", project_label(cwd)))
            .unwrap_or_default(),
        app.view.label(),
        rows.len()
    );
    let block = theme.block(title, app.panel == Panel::Sessions && !app.editing_search);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if !rows.is_empty() {
        let height = inner.height as usize;
        let offset = app.row.saturating_sub(height.saturating_sub(1));
        app.session_offset.set(offset);
        let items = rows
            .iter()
            .skip(offset)
            .take(height)
            .map(|s| {
                let mark = if !s.bindings.is_empty() {
                    "● "
                } else if !s.probable.is_empty() {
                    "≈ "
                } else {
                    "  "
                };
                let label = if app.project.is_none() {
                    format!(
                        "{} / {}",
                        project_label(&s.metadata.cwd),
                        s.metadata.title()
                    )
                } else {
                    s.metadata.title()
                };
                let resume = if s.state == crate::model::ResumeState::Resume {
                    "[R] "
                } else {
                    ""
                };
                ListItem::new(Line::from(vec![
                    Span::raw(mark),
                    Span::styled(resume, Style::new().fg(theme.focus).bold()),
                    Span::raw(paths::line(&label)),
                ]))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(Some(app.row.saturating_sub(offset)));
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("› ")
                .highlight_style(theme.selection),
            inner,
            &mut state,
        );
    } else {
        // Empty states teach the next move instead of just saying "nothing".
        let hint = if app.view == crate::model::View::Resume {
            "No sessions to resume.\n\nf → All explores the full history.\nNew sessions will appear here."
        } else {
            "No results.\n\nSearch another name or press Esc to clear.\nf changes the view"
        };
        frame.render_widget(
            Paragraph::new(hint)
                .wrap(Wrap { trim: false })
                .style(Style::new().fg(theme.muted)),
            inner,
        );
    }
    let unbound = app
        .catalog
        .unbound
        .iter()
        .filter(|p| app.project.as_ref().is_none_or(|c| *c == p.cwd))
        .filter(|p| {
            !app.catalog.sessions.iter().any(|s| {
                s.probable
                    .iter()
                    .any(|hint| hint.socket == p.socket && hint.pane == p.pane)
            })
        })
        .collect::<Vec<_>>();
    if !unbound.is_empty() && inner.height > 5 {
        let room = if rows.is_empty() {
            inner.height.saturating_sub(3)
        } else {
            inner.height.saturating_sub(rows.len() as u16 + 1)
        };
        let height = room.min(unbound.len() as u16 + 2);
        if height >= 3 {
            // Leftover space doubles as the unbound-pane strip: live Pi runs
            // with no conversation identity. `o` opens the pane picker.
            let text = std::iter::once("More open Pi runs — o browses panes".to_string())
                .chain(
                    unbound
                        .iter()
                        .map(|p| format!("  {}", paths::line(&p.title))),
                )
                .collect::<Vec<_>>()
                .join("\n");
            frame.render_widget(
                Paragraph::new(text).style(Style::new().fg(theme.warning)),
                Rect::new(inner.x, inner.bottom() - height, inner.width, height),
            );
        }
    }
}

fn detail(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let Some(session) = app.current() else {
        let text = "Pick a session\n\nEvery Pi conversation keeps its name,\nhistory, and your note.\n\nf → All explores every saved session.\n\nHistory and names are discovered automatically.\nOpenings without the extension show as probable.\n\nNo agent ever starts on its own.";
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(theme.block("Conversation".into(), app.panel == Panel::Detail)),
            area,
        );
        return;
    };
    let title = paths::line(&format!(
        "{} / {}",
        project_label(&session.metadata.cwd),
        session.metadata.title()
    ));
    let block = theme.block(title, app.panel == Panel::Detail && !app.editing_search);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.search_match.set(0);
    let mut matched_line = None;
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(session.state.label(), Style::new().fg(theme.focus).bold()),
            Span::raw(format!("    {}", session.presence())),
        ]),
        Line::from(Span::styled(
            project_label(&session.metadata.cwd),
            Style::new().fg(theme.muted),
        )),
        Line::from(Span::styled(
            format!("Last exchange: {}", paths::date(session.metadata.updated)),
            Style::new().fg(theme.muted),
        )),
    ];
    if !session.probable.is_empty() {
        lines.push(Line::styled(
            format!(
                "≈ {} probable pane(s) · Enter to choose",
                session.probable.len()
            ),
            Style::new().fg(theme.warning),
        ));
        lines.push(Line::styled(
            "Preview from the saved file, not the live Pi process.",
            Style::new().fg(theme.muted),
        ));
    }
    for binding in &session.bindings {
        if let Some(pane) = &binding.pane {
            lines.push(Line::from(format!(
                "{} / window {}",
                paths::line(&pane.session),
                paths::line(&pane.window)
            )));
        }
    }
    if session.changed_after_done() {
        lines.push(Line::styled(
            "Updated after completion",
            Style::new().fg(theme.warning),
        ));
    }
    if !session.available {
        lines.push(Line::styled(
            "Conversation unavailable",
            Style::new().fg(theme.warning),
        ));
    }
    lines.push(Line::default());
    if !session.note.is_empty() {
        lines.push(Line::styled("Next step", Style::new().bold()));
        for line in paths::clean(&session.note).lines() {
            lines.push(Line::raw(line.to_owned()));
        }
        lines.push(Line::default());
    }
    if app.detail_id.as_deref() != Some(&session.id) {
        lines.push(Line::styled(
            "Loading conversation…",
            Style::new().fg(theme.muted),
        ));
    } else {
        lines.push(Line::styled(
            if app.conversation.partial {
                "Partial preview"
            } else if app.branch_selected {
                "Chosen branch"
            } else if session
                .bindings
                .iter()
                .any(|b| b.bridge.leaf == app.conversation.leaf)
            {
                "Branch of an open Pi"
            } else {
                "Latest saved branch"
            },
            Style::new().fg(theme.muted),
        ));
        if !app.conversation.warning.is_empty() {
            lines.push(Line::styled(
                paths::line(&app.conversation.warning),
                Style::new().fg(theme.warning),
            ));
        }
        if app.conversation.pages > 1 {
            lines.push(Line::styled(
                format!(
                    "Page {} of {} (newest first)  [ older  ] newer",
                    app.conversation.page + 1,
                    app.conversation.pages
                ),
                Style::new().fg(theme.muted),
            ));
        }
        for message in &app.conversation.messages {
            lines.push(Line::default());
            lines.push(Line::styled(
                format!(
                    "{}  {}",
                    paths::line(&message.role),
                    paths::date(message.time)
                ),
                Style::new().fg(theme.focus).bold(),
            ));
            for text in paths::clean(&message.text).lines() {
                let style = if !app.detail_search.is_empty()
                    && text
                        .to_lowercase()
                        .contains(&app.detail_search.to_lowercase())
                {
                    if matched_line.is_none() {
                        matched_line = Some(lines.len());
                    }
                    theme.selection
                } else {
                    Style::new()
                };
                lines.push(Line::styled(text.to_owned(), style));
            }
        }
    }
    if let Some(index) = matched_line {
        app.search_match.set(
            Paragraph::new(lines[..index].to_vec())
                .wrap(Wrap { trim: false })
                .line_count(inner.width)
                .min(u16::MAX as usize) as u16,
        );
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    app.max_scroll.set(max_scroll);
    frame.render_widget(paragraph.scroll((app.scroll.min(max_scroll), 0)), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    pub fn fixture() -> App {
        let mut app = App {
            view: View::All,
            ..App::default()
        };
        app.catalog.sessions = [
            "website-redesign",
            "api-migration",
            "onboarding-flow",
            "perf-audit",
            "release-notes",
        ]
        .iter()
        .enumerate()
        .map(|(i, n)| Session {
            id: format!("fixture-{i}"),
            metadata: Metadata {
                name: n.to_string(),
                cwd: "/repo/acme-website".into(),
                ..Metadata::default()
            },
            state: ResumeState::Resume,
            note: if i == 0 {
                "Check the hero spacing in a real browser.".into()
            } else {
                String::new()
            },
            done_revision: 0,
            done_at: 0,
            available: true,
            presence_verified: false,
            bindings: vec![],
            probable: vec![],
        })
        .collect();
        app.refresh_filter();
        app.select(0);
        app.detail_id = app.selected.clone();
        app.conversation.messages = vec![
            Message {
                role: "You".into(),
                text: "Check the new landing copy.".into(),
                time: 0,
                tool: false,
            },
            Message {
                role: "Pi".into(),
                text: "The draft is ready for review.\nMobile spacing still needs checking.".into(),
                time: 0,
                tool: false,
            },
        ];
        app
    }
    #[test]
    fn resume_badge_is_independent_of_presence_and_visible_without_color() {
        use crate::model::{ResumeState, View};
        let mut app = fixture();
        app.project = Some("/repo/acme-website".into());
        app.view = View::All;
        app.catalog.sessions[0].state = ResumeState::Resume;
        app.catalog.sessions[1].state = ResumeState::History;
        app.catalog.sessions[2].state = ResumeState::Done;
        for session in &mut app.catalog.sessions[3..] {
            session.state = ResumeState::History;
        }
        app.no_color = true;
        app.refresh_filter();
        for (w, badge) in [(40, "[R]"), (160, "[R]")] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, 24)).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            let lines = (0..24)
                .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>();
            let row = lines
                .iter()
                .find(|line| line.contains("website-redesign") && line.contains(badge))
                .unwrap();
            assert!(row.contains(badge));
            assert!(!row.contains('≈') && !row.contains('●'));
            for name in ["release-notes", "api-migration"] {
                assert!(
                    !lines
                        .iter()
                        .find(|line| line.contains(name))
                        .unwrap()
                        .contains(badge)
                );
            }
            assert!(lines[22].contains("r to resume"));
            assert!(
                buffer
                    .content()
                    .iter()
                    .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
            );
        }
        app.panel = Panel::Detail;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("r to resume"));
    }
    #[test]
    fn project_counts_include_history_even_in_resume_view() {
        let mut app = fixture();
        for session in &mut app.catalog.sessions {
            session.state = crate::model::ResumeState::History;
        }
        app.view = crate::model::View::Resume;
        app.panel = Panel::Projects;
        app.refresh_filter();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("Acme Website"));
        assert!(text.contains("Search projects"));
        assert!(app.visible.is_empty());
        // Entering search clears the placeholder before the first keystroke,
        // so the cursor never renders inside the hint text.
        app.editing_search = true;
        let mut editing = Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
        editing.draw(|f| draw(f, &app)).unwrap();
        let editing_text = editing
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(!editing_text.contains("Name or folder"));
        assert!(editing_text.contains("Search projects"));
    }
    #[test]
    fn no_color_still_has_visible_selection() {
        let mut app = fixture();
        app.no_color = true;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|c| c.modifier.contains(Modifier::REVERSED))
        );
    }
    #[test]
    fn readable_contextual_action_menus() {
        for (w, h) in [(40, 12), (80, 24), (120, 32)] {
            for panel in [Panel::Projects, Panel::Sessions, Panel::Detail] {
                let mut app = fixture();
                app.panel = panel;
                let mut menu = super::super::menu::CommandMenu::new(&app);
                menu.selected = menu.actions.len() - 1;
                app.command_menu = Some(menu);
                let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
                terminal.draw(|f| draw(f, &app)).unwrap();
                let buffer = terminal.backend().buffer();
                let text = (0..h)
                    .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
                    .collect::<String>();
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/snapshots/menu-{panel:?}-{w}x{h}.txt"));
                if std::env::var("UPDATE_LAYOUT_SNAPSHOTS").as_deref() == Ok("1") {
                    std::fs::write(&path, &text).unwrap();
                }
                assert_eq!(text, std::fs::read_to_string(path).unwrap());
                assert!(text.contains("↑↓ choose"));
                assert!(text.contains("Refresh the catalog"));
            }
        }
    }
    #[test]
    fn project_identity_is_visible_without_technical_titles() {
        let mut app = fixture();
        for s in &mut app.catalog.sessions {
            s.metadata.cwd = "/repo/acme-website".into();
        }
        app.search = "acme website".into();
        app.refresh_filter();
        assert_eq!(app.rows().len(), 5);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("Acme Website / website-redesign"));
        let mut menu = super::super::menu::CommandMenu::new(&app);
        menu.selected = 0;
        app.command_menu = Some(menu);
        app.no_color = true;
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
        );
    }
    #[test]
    fn readable_layouts() {
        for (w, h) in [(40, 12), (80, 24), (120, 32), (160, 40)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            let app = fixture();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            let text = (0..h)
                .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
                .collect::<String>();
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/snapshots/layout-{w}x{h}.txt"));
            if std::env::var("UPDATE_LAYOUT_SNAPSHOTS").as_deref() == Ok("1") {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &text).unwrap();
            }
            assert_eq!(text, std::fs::read_to_string(path).unwrap());
            assert!(text.contains(if w < 65 { "websit" } else { "website-redesign" }));

            assert!(!text.contains("fixture-"));
            if w >= 80 {
                assert!(text.contains("Next step"));
                assert!(text.contains("You"));
            }
        }
    }
}
