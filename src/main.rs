//! CLI entry point: human names in the explorer, stable ids in scripts.
//!
//! With no subcommand (or `pick`) this opens the interactive TUI, which
//! requires a real terminal. Every other subcommand runs headlessly and
//! prints text or JSON. Starting Pi always needs an explicit confirmation
//! (`--resume` in scripts); verified/probable openings block duplicates.
use agent_monitor::{
    model::{ResumeState, View, project_name},
    paths, presence,
    service::Service,
    ui,
};
use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(version, about = "Your Pi sessions, ready to resume")]
struct Cli {
    #[arg(long, global = true, env = "AGENT_MONITOR_HOME")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Subcommand)]
enum Cmd {
    /// Open the explorer; nothing to create, just browse and resume.
    Pick {
        #[arg(long)]
        project: Option<String>,
    },
    /// Sessions to resume (or full history with --all).
    Sessions {
        #[arg(long)]
        project: Option<String>,
        #[arg(long,conflicts_with_all=["open","done"])]
        all: bool,
        #[arg(long, conflicts_with = "done")]
        open: bool,
        #[arg(long)]
        done: bool,
        #[arg(long, default_value = "")]
        search: String,
        #[arg(long)]
        json: bool,
    },
    /// Name, note, conversation, and openings of one session.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        tools: bool,
    },
    /// Jump to the open Pi; starting one needs explicit confirmation.
    Open {
        id: String,
        #[arg(long)]
        binding: Option<String>,
        #[arg(long)]
        resume: bool,
    },
    Done {
        id: String,
    },
    Reopen {
        id: String,
    },
    Note {
        id: String,
        text: String,
    },
    Undo {
        id: String,
    },
    /// Refresh the catalog without starting or touching any agent.
    Sync,
    Sources {
        #[command(subcommand)]
        command: Sources,
    },
    Integration {
        #[command(subcommand)]
        command: Integration,
    },
}
#[derive(Subcommand)]
enum Sources {
    List,
    Add { path: PathBuf },
}
#[derive(Subcommand)]
enum Integration {
    Pi {
        #[command(subcommand)]
        command: PiIntegration,
    },
}
#[derive(Subcommand)]
enum PiIntegration {
    Status,
    Install,
    Remove,
}
fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("agent-monitor: {}", paths::line(&format!("{e:#}")));
        std::process::exit(1);
    }
}
fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn run(cli: Cli) -> Result<()> {
    let command = match cli.command {
        None => return ui::run(paths::root(cli.data_dir)?, None),
        Some(Cmd::Pick { project }) => return ui::run(paths::root(cli.data_dir)?, project),
        Some(command) => command,
    };
    let service = Service::open(cli.data_dir)?;
    match command {
        Cmd::Pick { .. } => unreachable!("picker handled before opening SQLite"),
        Cmd::Sources { command } => match command {
            Sources::List => {
                for source in service.store.sources()? {
                    println!(
                        "{}{}",
                        paths::line(&source.path.to_string_lossy()),
                        if source.complete {
                            ""
                        } else {
                            " (indexing incomplete)"
                        }
                    );
                }
                Ok(())
            }
            Sources::Add { path } => service.store.add_source(&path),
        },
        Cmd::Integration {
            command: Integration::Pi { command },
        } => match command {
            PiIntegration::Status => {
                let path = presence::installed_path();
                println!(
                    "{}: {}",
                    if path.exists() {
                        "Installed"
                    } else {
                        "Not installed"
                    },
                    paths::line(&path.to_string_lossy())
                );
                Ok(())
            }
            PiIntegration::Install => {
                let path = presence::install()?;
                println!(
                    "Installed: {}\nRun /reload in every Pi that is already open.",
                    paths::line(&path.to_string_lossy())
                );
                Ok(())
            }
            PiIntegration::Remove => {
                println!(
                    "Removed: {}",
                    paths::line(&presence::remove()?.to_string_lossy())
                );
                Ok(())
            }
        },
        Cmd::Sync => {
            for warning in service.sync()? {
                eprintln!("{}", paths::line(&warning));
            }
            let catalog = service.catalog()?;
            println!(
                "{} conversations, {} unassociated Pi panes",
                catalog.sessions.len(),
                catalog.unbound.len()
            );
            Ok(())
        }
        Cmd::Sessions {
            project,
            all,
            open,
            done,
            search,
            json,
        } => {
            service.sync()?;
            let catalog = service.catalog()?;
            let project = resolve_project(&catalog, project)?;
            let view = if all {
                View::All
            } else if open {
                View::Open
            } else if done {
                View::Done
            } else {
                View::Resume
            };
            let mut app = ui::App {
                catalog,
                project,
                view,
                search,
                ..Default::default()
            };
            app.refresh_filter();
            let rows = app.rows();
            if json {
                return print_json(&rows);
            }
            for s in &rows {
                println!(
                    "{}  {}  {}\n  {}",
                    paths::line(&s.metadata.title()),
                    s.state.label(),
                    s.presence(),
                    s.id
                );
            }
            if rows.is_empty() {
                println!("No sessions in {}. Use --all for history.", view.label());
            }
            if !app.catalog.unbound.is_empty() {
                println!(
                    "\n{} unassociated Pi panes: integration pi install, then /reload.",
                    app.catalog.unbound.len()
                );
            }
            Ok(())
        }
        Cmd::Show { id, json, tools } => {
            service.sync()?;
            let catalog = service.catalog()?;
            let s = service.resolve(&catalog, &id)?;
            let leaf = s.bindings.first().and_then(|b| b.bridge.leaf.as_deref());
            let conversation = if s.available {
                service.detail(&s.id, leaf, tools)?
            } else {
                agent_monitor::model::Conversation {
                    warning: "Conversation unavailable; name and notes kept".into(),
                    ..Default::default()
                }
            };
            if json {
                return print_json(&serde_json::json!({"session":s,"conversation":conversation}));
            }
            println!(
                "{}\n{} · {}\n{}",
                paths::line(&s.metadata.title()),
                s.state.label(),
                s.presence(),
                paths::line(&s.metadata.cwd)
            );
            if s.changed_after_done() {
                println!("Updated after completion");
            }
            if !conversation.warning.is_empty() {
                println!("{}", paths::line(&conversation.warning));
            }
            if !s.note.is_empty() {
                println!("\nNext step\n{}", paths::clean(&s.note));
            }
            for message in conversation.messages {
                println!("\n{}\n{}", message.role, paths::clean(&message.text));
            }
            Ok(())
        }
        Cmd::Done { id } => service.store.change(&id, Some(ResumeState::Done), None),
        Cmd::Reopen { id } => service.store.change(&id, Some(ResumeState::Resume), None),
        Cmd::Note { id, text } => service.store.change(&id, None, Some(&text)),
        Cmd::Undo { id } => service.store.undo(&id),
        Cmd::Open {
            id,
            binding,
            resume,
        } => {
            service.sync()?;
            let catalog = service.catalog()?;
            let s = service.resolve(&catalog, &id)?;
            if !s.bindings.is_empty() {
                let chosen = if let Some(nonce) = binding {
                    s.bindings
                        .iter()
                        .find(|b| b.bridge.nonce == nonce)
                        .ok_or_else(|| anyhow::anyhow!("opening not found"))?
                } else {
                    ensure!(
                        s.bindings.len() == 1,
                        "multiple openings: pass --binding with the nonce from show --json"
                    );
                    &s.bindings[0]
                };
                return presence::focus(chosen);
            }
            let spec = service.resume_spec(&s.id)?;
            if !spec.warning.is_empty() {
                eprintln!("{}", paths::line(&spec.warning));
            }
            if !resume {
                ensure!(
                    io::stdin().is_terminal() && io::stdout().is_terminal(),
                    "pass --resume to authorize a non-interactive Pi start"
                );
                print!(
                    "Resume '{}' in a new tmux window/session, folder {}? [y/N] ",
                    paths::line(&s.metadata.title()),
                    paths::line(&spec.cwd)
                );
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim(), "s" | "S" | "y" | "Y") {
                    return Ok(());
                }
            }
            let fresh = service.resume_spec(&s.id)?;
            ensure!(
                resume || !spec.warning.is_empty() || fresh.warning.is_empty(),
                "Presence changed: repeat the command and confirm the new situation"
            );
            fresh.launch()?;
            service.store.manage(&fresh.file)
        }
    }
}
fn resolve_project(
    catalog: &agent_monitor::model::Catalog,
    project: Option<String>,
) -> Result<Option<String>> {
    let Some(project) = project else {
        return Ok(None);
    };
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
                || project_name(p) == project
                || agent_monitor::model::project_label(p).eq_ignore_ascii_case(&project)
        })
        .collect::<Vec<_>>();
    ensure!(
        matches.len() == 1,
        "project not found or ambiguous: use the full path"
    );
    Ok(matches.into_iter().next())
}
