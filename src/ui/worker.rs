//! Background worker: blocking I/O lives here, never in render/input.
//!
//! Two lanes with different semantics: detail reads are latest-wins (only
//! the newest query is answered, stale ones are dropped), while mutations
//! are an ordered queue where every write gets an explicit ack. The status
//! line reports "Saved" only when that ack arrives.
use crate::{
    model::*,
    presence,
    service::{ResumeSpec, Service},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct DetailQuery {
    pub id: String,
    pub generation: u64,
    pub leaf: Option<String>,
    pub tools: bool,
    pub page: usize,
}
pub enum Action {
    Change {
        id: String,
        state: Option<ResumeState>,
        note: Option<String>,
    },
    Undo(String),
    Managed(PathBuf),
    Focus(Box<Binding>),
    FocusPane(Box<crate::tmux::PaneIdentity>),
    Prepare(String),
    Resume {
        id: String,
        allow_uncertain: bool,
    },
    Preferences {
        project: String,
        view: View,
    },
    Refresh,
}
pub enum Reply {
    Catalog(Catalog),
    Detail {
        query: DetailQuery,
        result: Result<Conversation, String>,
    },
    Ack(Result<String, String>),
    Confirm {
        id: String,
        spec: ResumeSpec,
    },
    Launch(ResumeSpec),
    Preferences {
        project: Option<String>,
        view: View,
    },
}
/// Handles into the worker thread: ordered action queue, latest detail
/// query slot, and the reply channel the event loop drains each frame.
pub struct Worker {
    pub actions: mpsc::SyncSender<Action>,
    pub queries: Arc<Mutex<Option<DetailQuery>>>,
    pub replies: mpsc::Receiver<Reply>,
}
impl Worker {
    #[cfg(test)]
    pub(super) fn idle() -> Self {
        let (actions, _) = mpsc::sync_channel(16);
        let (_, replies) = mpsc::sync_channel(16);
        Self {
            actions,
            queries: Arc::new(Mutex::new(None)),
            replies,
        }
    }
    pub fn start(root: PathBuf) -> Self {
        let (actions, rx) = mpsc::sync_channel(16);
        let (tx, replies) = mpsc::sync_channel(16);
        let queries = Arc::new(Mutex::new(None));
        let queries_worker = queries.clone();
        std::thread::spawn(move || run(root, rx, queries_worker, tx));
        Self {
            actions,
            queries,
            replies,
        }
    }
    pub fn query(&self, query: DetailQuery) {
        *self.queries.lock().unwrap() = Some(query);
    }
}
fn error(e: anyhow::Error) -> String {
    crate::paths::line(&format!("{e:#}"))
}
fn run(
    root: PathBuf,
    actions: mpsc::Receiver<Action>,
    queries: Arc<Mutex<Option<DetailQuery>>>,
    replies: mpsc::SyncSender<Reply>,
) {
    let service = match Service::open(Some(root)) {
        Ok(s) => s,
        Err(e) => {
            let _ = replies.send(Reply::Ack(Err(error(e))));
            return;
        }
    };
    let project = service
        .store
        .preference("project")
        .ok()
        .flatten()
        .filter(|p| !p.is_empty());
    let view = service
        .store
        .preference("explorer_view")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or(View::All);
    if replies.send(Reply::Preferences { project, view }).is_err() {
        return;
    }
    let mut catalog = Catalog {
        sessions: service.store.list().unwrap_or_default(),
        scanning: true,
        ..Default::default()
    };
    if replies.send(Reply::Catalog(catalog.clone())).is_err() {
        return;
    }
    let mut indexer = match service.begin_index() {
        Ok(i) => Some(i),
        Err(e) => {
            let _ = replies.send(Reply::Ack(Err(error(e))));
            None
        }
    };
    let mut last_presence = Instant::now() - Duration::from_secs(3);
    let mut last_catalog = Instant::now();
    let mut last_index = Instant::now();
    loop {
        // One mutation per loop turn; reads can never consume a pending write.
        match actions.try_recv() {
            Ok(action) => {
                let reply = match action {
                    Action::Change { id, state, note } => Reply::Ack(
                        service
                            .store
                            .change(&id, state, note.as_deref())
                            .map(|()| "Saved".into())
                            .map_err(error),
                    ),
                    Action::Undo(id) => Reply::Ack(
                        service
                            .store
                            .undo(&id)
                            .map(|()| "Change undone".into())
                            .map_err(error),
                    ),
                    Action::Managed(file) => Reply::Ack(
                        service
                            .store
                            .manage(&file)
                            .map(|()| String::new())
                            .map_err(error),
                    ),
                    Action::Focus(binding) => Reply::Ack(
                        presence::focus(&binding)
                            .map(|()| "Opening selected".into())
                            .map_err(error),
                    ),
                    Action::FocusPane(pane) => Reply::Ack(
                        crate::tmux::focus_probable(&pane)
                            .map(|()| "Pane selected (association unverified)".into())
                            .map_err(error),
                    ),
                    Action::Prepare(id) => match service.resume_spec(&id) {
                        Ok(spec) => Reply::Confirm { id, spec },
                        Err(e) => Reply::Ack(Err(error(e))),
                    },
                    Action::Resume {
                        id,
                        allow_uncertain,
                    } => match service.resume_spec(&id) {
                        Ok(spec) if !spec.warning.is_empty() && !allow_uncertain => {
                            Reply::Confirm { id, spec }
                        }
                        Ok(spec) => Reply::Launch(spec),
                        Err(e) => Reply::Ack(Err(error(e))),
                    },
                    Action::Preferences { project, view } => Reply::Ack(
                        service
                            .store
                            .set_preference("project", &project)
                            .and_then(|()| {
                                service
                                    .store
                                    .set_preference("explorer_view", &serde_json::to_string(&view)?)
                            })
                            .map(|()| String::new())
                            .map_err(error),
                    ),
                    Action::Refresh => {
                        last_presence = Instant::now() - Duration::from_secs(3);
                        last_index = Instant::now() - Duration::from_secs(6);
                        Reply::Ack(Ok("Refresh requested".into()))
                    }
                };
                if replies.send(reply).is_err() {
                    break;
                }
                last_catalog = Instant::now() - Duration::from_secs(1);
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let query = queries.lock().unwrap().take();
        if let Some(query) = query {
            let result = service
                .detail_page(&query.id, query.leaf.as_deref(), query.tools, query.page)
                .map_err(error);
            if replies.send(Reply::Detail { query, result }).is_err() {
                break;
            }
        }
        if last_presence.elapsed() >= Duration::from_secs(2) {
            match service.catalog() {
                Ok(new) => catalog = new,
                Err(e) => {
                    catalog.warnings = vec![error(e)];
                    for session in &mut catalog.sessions {
                        session.presence_verified = false;
                        session.probable.clear();
                        session
                            .bindings
                            .retain(|b| (0..8000).contains(&(crate::paths::now() - b.bridge.seen)));
                    }
                }
            }
            last_presence = Instant::now();
            last_catalog = Instant::now() - Duration::from_secs(1);
        }
        if let Some(index) = &mut indexer {
            if let Err(e) = service.index_step(index) {
                catalog.warnings.push(error(e));
                indexer = None;
            } else if !index.pending() {
                for warning in &index.warnings {
                    if !catalog.warnings.contains(warning) {
                        catalog.warnings.push(warning.clone());
                    }
                }
                indexer = None;
                last_catalog = Instant::now() - Duration::from_secs(1);
            }
        }
        if indexer.is_none() && last_index.elapsed() >= Duration::from_secs(5) {
            indexer = service.begin_index().ok();
            last_index = Instant::now();
        }
        if last_catalog.elapsed() >= Duration::from_millis(200) {
            if let Ok(mut sessions) = service.store.list() {
                let previous = catalog
                    .sessions
                    .iter()
                    .map(|s| (s.id.as_str(), s))
                    .collect::<std::collections::HashMap<_, _>>();
                for session in &mut sessions {
                    if let Some(previous) = previous.get(session.id.as_str()) {
                        session.bindings = previous.bindings.clone();
                        session.presence_verified = previous.presence_verified;
                        session.live_name();
                    }
                }
                sessions.extend(
                    catalog
                        .sessions
                        .iter()
                        .filter(|s| s.id.starts_with("live-"))
                        .cloned(),
                );
                crate::service::associate_probable(&mut sessions, &catalog.unbound);
                catalog.sessions = sessions;
            }
            catalog.scanning = indexer.as_ref().is_some_and(|i| i.initial && i.pending());
            if let Some(index) = &indexer {
                for warning in &index.warnings {
                    if !catalog.warnings.contains(warning) {
                        catalog.warnings.push(warning.clone());
                    }
                }
            }
            if replies.send(Reply::Catalog(catalog.clone())).is_err() {
                break;
            }
            last_catalog = Instant::now();
        }
        if indexer.is_none() {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detail_slot_is_latest_wins_without_consuming_actions() {
        let slot = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel(16);
        tx.send(Action::Undo("first".into())).unwrap();
        tx.send(Action::Undo("second".into())).unwrap();
        for generation in 0..100 {
            *slot.lock().unwrap() = Some(DetailQuery {
                id: generation.to_string(),
                generation,
                leaf: None,
                tools: false,
                page: 0,
            });
        }
        assert_eq!(slot.lock().unwrap().take().unwrap().generation, 99);
        assert!(matches!(rx.recv().unwrap(),Action::Undo(id) if id=="first"));
        assert!(matches!(rx.recv().unwrap(),Action::Undo(id) if id=="second"));
    }
}
