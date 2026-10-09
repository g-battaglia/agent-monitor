//! Project presentation only: folder hierarchy, folding, layouts, and sorting.
//! Identity and work decisions stay attached to canonical folders/sessions.
use crate::{
    model::{Catalog, View, project_label},
    paths,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProjectLayout {
    #[default]
    Tree,
    Flat,
}
impl ProjectLayout {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tree => "Tree",
            Self::Flat => "Flat",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProjectOrder {
    #[default]
    Alphabetical,
    Recent,
}
impl ProjectOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Alphabetical => "A–Z",
            Self::Recent => "Recent",
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct ProjectOptions {
    pub layout: ProjectLayout,
    pub order: ProjectOrder,
    pub collapsed: BTreeSet<String>,
}
#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub cwd: String,
    /// Navigation-only ancestor directory, not a saved project/session.
    pub is_group: bool,
    pub label: String,
    pub depth: usize,
    pub parent: Option<String>,
    pub shown: usize,
    pub total: usize,
    pub latest: i64,
    pub has_children: bool,
    pub collapsed: bool,
    pub descendants: usize,
}
#[derive(Debug, Clone, Default)]
pub struct ProjectTree {
    pub root: String,
    pub rows: Vec<ProjectRow>,
    /// Includes branches hidden beneath another closed branch.
    pub branches: Vec<String>,
    pub project_count: usize,
    pub known_timestamps: usize,
}
pub fn folder(path: &str) -> String {
    if path.is_empty() {
        return "Unknown folder".into();
    }
    let home = paths::expand_home(Path::new("~"));
    if let Ok(tail) = Path::new(path).strip_prefix(home) {
        return if tail.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", tail.display())
        };
    }
    paths::line(path)
}

fn base_tree(
    catalog: &Catalog,
    view: View,
    selected: Option<&str>,
    search: &str,
    layout: ProjectLayout,
) -> ProjectTree {
    let mut counts = BTreeMap::<String, (usize, usize, i64)>::new();
    for s in &catalog.sessions {
        let count = counts.entry(s.metadata.cwd.clone()).or_default();
        count.1 += 1;
        count.0 += usize::from(view.matches(s));
        count.2 = count.2.max(s.metadata.updated);
    }
    let needle = search.to_lowercase();
    let mut rows = counts
        .iter()
        .filter(|(cwd, (shown, _, _))| {
            (*shown > 0 || selected == Some(cwd.as_str()))
                && (cwd.to_lowercase().contains(&needle)
                    || project_label(cwd).to_lowercase().contains(&needle))
        })
        .map(|(cwd, (shown, total, latest))| ProjectRow {
            cwd: cwd.clone(),
            is_group: false,
            label: String::new(),
            depth: 0,
            parent: None,
            shown: *shown,
            total: *total,
            latest: *latest,
            has_children: false,
            collapsed: false,
            descendants: 0,
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| (a.cwd.is_empty(), &a.cwd).cmp(&(b.cwd.is_empty(), &b.cwd)));
    // A stable root across views/searches keeps fold state meaningful in Open.
    let known = counts
        .keys()
        .filter(|cwd| Path::new(cwd).is_absolute())
        .collect::<Vec<_>>();
    let first = known.first().map(Path::new).unwrap_or(Path::new("/"));
    let mut common = if known.len() > 1 {
        first
    } else {
        first.parent().unwrap_or(first)
    }
    .to_path_buf();
    for cwd in &known {
        while !Path::new(cwd).starts_with(&common) {
            if !common.pop() {
                break;
            }
        }
    }
    let project_count = rows.len();
    let known_timestamps = rows.iter().filter(|r| r.latest > 0).count();
    if layout == ProjectLayout::Tree {
        let indexed = rows.iter().map(|r| r.cwd.clone()).collect::<BTreeSet<_>>();
        let mut groups = BTreeMap::<String, (usize, usize, i64)>::new();
        for row in &rows {
            if !Path::new(&row.cwd).is_absolute() {
                continue;
            }
            for parent in Path::new(&row.cwd)
                .ancestors()
                .skip(1)
                .take_while(|p| p.starts_with(&common))
            {
                let path = parent.to_string_lossy().into_owned();
                if !indexed.contains(&path) {
                    groups.entry(path).or_default();
                }
            }
        }
        // Aggregate once per project/ancestor, not per folder × transcript.
        for (cwd, (shown, total, latest)) in &counts {
            if !Path::new(cwd).is_absolute() {
                continue;
            }
            for parent in Path::new(cwd)
                .ancestors()
                .take_while(|p| p.starts_with(&common))
            {
                if let Some(group) = groups.get_mut(parent.to_string_lossy().as_ref()) {
                    group.0 += shown;
                    group.1 += total;
                    group.2 = group.2.max(*latest);
                }
            }
        }
        rows.extend(
            groups
                .into_iter()
                .map(|(cwd, (shown, total, latest))| ProjectRow {
                    cwd,
                    is_group: true,
                    label: String::new(),
                    depth: 0,
                    parent: None,
                    shown,
                    total,
                    latest,
                    has_children: false,
                    collapsed: false,
                    descendants: 0,
                }),
        );
        rows.sort_by(|a, b| (a.cwd.is_empty(), &a.cwd).cmp(&(b.cwd.is_empty(), &b.cwd)));
    }
    let mut stack = Vec::<(PathBuf, usize)>::new();
    for row in &mut rows {
        let cwd = Path::new(&row.cwd);
        while stack.last().is_some_and(|(parent, _)| {
            row.cwd.is_empty() || !cwd.starts_with(parent) || cwd == parent
        }) {
            stack.pop();
        }
        let relative = if let Some((parent, depth)) = stack.last() {
            row.depth = depth + 1;
            row.parent = Some(parent.to_string_lossy().into_owned());
            cwd.strip_prefix(parent).unwrap_or(cwd)
        } else {
            cwd.strip_prefix(&common).unwrap_or(cwd)
        };
        row.label = if row.cwd.is_empty() {
            "Unknown folder".into()
        } else if cwd == paths::expand_home(Path::new("~")) {
            "~".into()
        } else if relative.as_os_str().is_empty() {
            project_label(&row.cwd)
        } else {
            let prefix = relative
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| format!("{}/", p.display()))
                .unwrap_or_default();
            format!("{prefix}{}", project_label(&row.cwd))
        };
        if !row.cwd.is_empty() {
            stack.push((cwd.to_path_buf(), row.depth));
        }
    }
    let root = if rows.is_empty() {
        String::new()
    } else if rows.iter().all(|r| r.cwd.is_empty()) {
        "Unknown folder".into()
    } else {
        folder(&common.to_string_lossy())
    };
    ProjectTree {
        root,
        rows,
        branches: vec![],
        project_count,
        known_timestamps,
    }
}

pub fn build(catalog: &Catalog, view: View, selected: Option<&str>, search: &str) -> ProjectTree {
    build_with_options(catalog, view, selected, search, &ProjectOptions::default())
}
pub fn build_with_options(
    catalog: &Catalog,
    view: View,
    selected: Option<&str>,
    search: &str,
    options: &ProjectOptions,
) -> ProjectTree {
    let mut tree = base_tree(catalog, view, selected, search, options.layout);
    let indices = tree
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r.cwd.clone(), i))
        .collect::<BTreeMap<_, _>>();
    let mut children = vec![vec![]; tree.rows.len()];
    let mut roots = vec![];
    let mut parents = vec![None; tree.rows.len()];
    for (i, row) in tree.rows.iter().enumerate() {
        if let Some(parent) = row.parent.as_ref().and_then(|p| indices.get(p)).copied() {
            children[parent].push(i);
            parents[i] = Some(parent);
        } else {
            roots.push(i);
        }
    }
    let mut latest = tree.rows.iter().map(|r| r.latest).collect::<Vec<_>>();
    // Original rows are parent-first; propagate subtree recency and size upward.
    for i in (0..tree.rows.len()).rev() {
        tree.rows[i].has_children = !children[i].is_empty();
        tree.rows[i].collapsed = tree.rows[i].has_children
            && search.is_empty()
            && options.collapsed.contains(&tree.rows[i].cwd);
        if let Some(parent) = parents[i] {
            latest[parent] = latest[parent].max(latest[i]);
            tree.rows[parent].descendants +=
                tree.rows[i].descendants + usize::from(!tree.rows[i].is_group);
        }
    }
    tree.branches = tree
        .rows
        .iter()
        .filter(|r| r.has_children)
        .map(|r| r.cwd.clone())
        .collect();
    if options.layout == ProjectLayout::Flat {
        let mut duplicates = BTreeMap::<String, usize>::new();
        for row in &tree.rows {
            *duplicates
                .entry(project_label(&row.cwd).to_lowercase())
                .or_default() += 1;
        }
        for row in &mut tree.rows {
            let name = project_label(&row.cwd);
            row.label = if duplicates[&name.to_lowercase()] > 1 {
                let parent = Path::new(&row.cwd)
                    .parent()
                    .map(|p| folder(&p.to_string_lossy()))
                    .unwrap_or_default();
                format!("{name} ({parent})")
            } else {
                name
            };
            row.depth = 0;
            row.parent = None;
            row.has_children = false;
            row.collapsed = false;
        }
        tree.rows
            .sort_by(|a, b| compare(a, b, a.latest, b.latest, options.order));
        return tree;
    }
    let cmp = |a: &usize, b: &usize| {
        compare(
            &tree.rows[*a],
            &tree.rows[*b],
            latest[*a],
            latest[*b],
            options.order,
        )
    };
    roots.sort_by(cmp);
    for group in &mut children {
        group.sort_by(cmp);
    }
    let mut pending = roots.into_iter().rev().collect::<Vec<_>>();
    let mut rows = Vec::with_capacity(tree.rows.len());
    while let Some(i) = pending.pop() {
        let row = &tree.rows[i];
        if !row.collapsed {
            pending.extend(children[i].iter().rev().copied());
        }
        rows.push(row.clone());
    }
    tree.rows = rows;
    tree
}
fn compare(
    a: &ProjectRow,
    b: &ProjectRow,
    a_latest: i64,
    b_latest: i64,
    order: ProjectOrder,
) -> Ordering {
    a.cwd
        .is_empty()
        .cmp(&b.cwd.is_empty())
        .then_with(|| {
            if order == ProjectOrder::Recent {
                b_latest.cmp(&a_latest)
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        .then_with(|| a.cwd.cmp(&b.cwd))
}
impl ProjectTree {
    pub fn label(&self, index: usize) -> String {
        let row = &self.rows[index];
        let marker = if row.has_children {
            if row.collapsed { "▸ " } else { "▾ " }
        } else {
            ""
        };
        let hidden = if row.collapsed {
            format!(" ({} hidden)", row.descendants)
        } else {
            String::new()
        };
        let suffix = if row.is_group && !row.label.ends_with('/') {
            "/"
        } else {
            ""
        };
        let label = format!("{marker}{}{suffix}{hidden}", paths::line(&row.label));
        if row.depth == 0 {
            return label;
        }
        let has_sibling = |index: usize, depth: usize| {
            self.rows
                .iter()
                .skip(index + 1)
                .take_while(|r| r.depth >= depth)
                .any(|r| r.depth == depth)
        };
        let mut guides = String::new();
        for level in 1..row.depth {
            let ancestor = (0..index)
                .rev()
                .find(|i| self.rows[*i].depth == level)
                .unwrap();
            guides.push_str(if has_sibling(ancestor, level) {
                "│  "
            } else {
                "   "
            });
        }
        format!(
            "{guides}{} {label}",
            if has_sibling(index, row.depth) {
                "├─"
            } else {
                "└─"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    fn session(cwd: &str) -> Session {
        Session {
            id: cwd.into(),
            metadata: Metadata {
                cwd: cwd.into(),
                ..Default::default()
            },
            state: ResumeState::History,
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
    fn tree_uses_real_ancestry_and_keeps_duplicate_folder_names_distinct() {
        let catalog = Catalog {
            sessions: vec![
                session("/repo/T"),
                session("/repo/T/Python"),
                session("/repo/Tools/Python"),
                session("/repo/Together"),
                session(""),
            ],
            ..Default::default()
        };
        let tree = build(&catalog, View::All, None, "");
        assert_eq!(tree.root, "/repo");
        let python = tree
            .rows
            .iter()
            .position(|r| r.cwd == "/repo/T/Python")
            .unwrap();
        assert!(tree.label(python).ends_with("└─ Python"));
        assert_eq!(tree.rows[python].parent.as_deref(), Some("/repo/T"));
        assert!(
            tree.rows
                .iter()
                .find(|r| r.cwd == "/repo/Together")
                .is_some_and(|r| r.parent.as_deref() == Some("/repo"))
        );
        assert!(
            tree.rows.iter().any(
                |r| r.cwd == "/repo/Tools/Python" && r.parent.as_deref() == Some("/repo/Tools")
            )
        );
        assert_eq!(tree.rows.last().unwrap().label, "Unknown folder");
        assert_eq!(tree.rows.last().unwrap().depth, 0);
        let catalog = Catalog {
            sessions: vec![
                session("/repo/app"),
                session("/repo/app/a"),
                session("/repo/app/a/deep"),
                session("/repo/app/b"),
            ],
            ..Default::default()
        };
        let tree = build(&catalog, View::All, None, "");
        assert_eq!(tree.root, "/repo/app");
        assert_eq!(tree.label(0), "▾ App");
        assert_eq!(tree.label(1), "├─ ▾ A");
        assert_eq!(tree.label(2), "│  └─ Deep");
        assert_eq!(tree.label(3), "└─ B");
    }
    #[test]
    fn projects_follow_the_view_and_keep_a_selected_empty_project_visible() {
        let catalog = Catalog {
            sessions: vec![session("/repo/archive"), session("/repo/current")],
            ..Default::default()
        };
        assert!(build(&catalog, View::Open, None, "").rows.is_empty());
        let tree = build(&catalog, View::Open, Some("/repo/current"), "");
        assert_eq!(tree.rows.iter().filter(|r| !r.is_group).count(), 1);
        let row = tree.rows.iter().find(|r| !r.is_group).unwrap();
        assert_eq!((row.shown, row.total), (0, 1));
        let tree = build(&catalog, View::All, None, "archive");
        assert_eq!(tree.rows.iter().filter(|r| !r.is_group).count(), 1);
        assert!(
            tree.rows
                .iter()
                .any(|r| r.cwd == "/repo/archive" && !r.is_group)
        );
    }
    #[test]
    fn folding_hides_descendants_but_search_reveals_them_without_clearing_folds() {
        let catalog = Catalog {
            sessions: vec![
                session("/repo/app"),
                session("/repo/app/api"),
                session("/repo/app/api/tests"),
                session("/repo/tools"),
            ],
            ..Default::default()
        };
        let mut options = ProjectOptions::default();
        options.collapsed.insert("/repo/app".into());
        let tree = build_with_options(&catalog, View::All, None, "", &options);
        assert_eq!(tree.rows.len(), 3);
        assert!(tree.label(1).ends_with("▸ App (2 hidden)"));
        assert!(tree.branches.contains(&"/repo/app/api".into()));
        let search = build_with_options(&catalog, View::All, None, "tests", &options);
        assert_eq!(search.rows.iter().filter(|r| !r.is_group).count(), 1);
        assert!(
            search
                .rows
                .iter()
                .any(|r| r.cwd == "/repo/app/api/tests" && !r.is_group)
        );
        assert!(options.collapsed.contains("/repo/app"));
        options.layout = ProjectLayout::Flat;
        let flat = build_with_options(&catalog, View::All, None, "", &options);
        assert_eq!(flat.rows.len(), 4);
        assert!(flat.rows.iter().all(|r| r.depth == 0 && !r.collapsed));
    }
    #[test]
    fn recent_tree_sort_keeps_children_with_parents_and_flat_sort_uses_each_project() {
        let mut rows = vec![
            session("/repo/alpha"),
            session("/repo/alpha/child"),
            session("/repo/zebra"),
            session("/repo/zebra/child"),
        ];
        rows[0].metadata.updated = 10;
        rows[1].metadata.updated = 20;
        rows[2].metadata.updated = 5;
        rows[3].metadata.updated = 100;
        let catalog = Catalog {
            sessions: rows,
            ..Default::default()
        };
        let mut options = ProjectOptions {
            order: ProjectOrder::Recent,
            ..Default::default()
        };
        let tree = build_with_options(&catalog, View::All, None, "", &options);
        assert_eq!(
            tree.rows
                .iter()
                .filter(|r| !r.is_group)
                .map(|r| r.cwd.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/repo/zebra",
                "/repo/zebra/child",
                "/repo/alpha",
                "/repo/alpha/child"
            ]
        );
        options.layout = ProjectLayout::Flat;
        let flat = build_with_options(&catalog, View::All, None, "", &options);
        assert_eq!(flat.rows[0].cwd, "/repo/zebra/child");
        assert_eq!(flat.rows[1].cwd, "/repo/alpha/child");
        assert!(flat.rows[0].label.contains("/repo/zebra"));
        options.order = ProjectOrder::Alphabetical;
        let flat = build_with_options(&catalog, View::All, None, "", &options);
        assert_eq!(flat.rows[0].label, "Alpha");
    }
}
