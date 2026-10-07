//! Deterministic preview with synthetic data.
//!
//! Renders one fixed catalog to stdout so layout changes stay visible in
//! review. No Pi, tmux, disk catalog, or real user data is ever touched.
use agent_monitor::{
    model::*,
    ui::{App, render},
};
use ratatui::{Terminal, backend::TestBackend};
fn main() -> anyhow::Result<()> {
    let mut app = App {
        view: View::All,
        ..App::default()
    };
    // Generic demo names on purpose: nothing here should look like a real
    // client project, session, or folder from anyone's machine.
    app.catalog.sessions = [
        "website-redesign",
        "api-migration",
        "onboarding-flow",
        "perf-audit",
        "new-landing",
        "billing-trial",
        "release-notes",
        "search-tuning",
        "mobile-spacing",
        "signup-copy",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, name)| Session {
        id: format!("demo-{i}"),
        metadata: Metadata {
            name: name.into(),
            cwd: "/repo/acme-website".into(),
            ..Default::default()
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
    let mut terminal = Terminal::new(TestBackend::new(120, 32))?;
    terminal.draw(|frame| render::draw(frame, &app))?;
    let buffer = terminal.backend().buffer();
    for y in 0..32 {
        for x in 0..120 {
            print!("{}", buffer[(x, y)].symbol());
        }
        println!();
    }
    Ok(())
}
