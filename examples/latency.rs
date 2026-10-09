//! Synthetic render benchmark: 10k sessions, 100 projects, one big page.
use ratatui::{Terminal, backend::TestBackend};
use std::time::Instant;
use tmux_agent_monitor::{
    model::*,
    ui::{App, render},
};
fn main() -> anyhow::Result<()> {
    let mut app = App {
        view: View::All,
        ..Default::default()
    };
    app.catalog.sessions = (0..10_000)
        .map(|i| Session {
            id: format!("fixture-{i}"),
            metadata: Metadata {
                name: format!("Work session {i}"),
                cwd: format!("/repo/project-{}", i % 100),
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
        })
        .collect();
    app.refresh_filter();
    app.select(0);
    app.conversation.messages = (0..100)
        .map(|i| Message {
            role: "Pi".into(),
            text: format!(
                "Message {i}\n{}",
                "Synthetic text for measuring rendering. ".repeat(80)
            ),
            time: 0,
            tool: false,
        })
        .collect();
    let mut terminal = Terminal::new(TestBackend::new(120, 32))?;
    let mut times = vec![];
    for i in 0..100 {
        let start = Instant::now();
        app.select(i);
        app.detail_id = app.selected.clone();
        terminal.draw(|f| render::draw(f, &app))?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "10,000 sessions / 100 projects / large page: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        times[50], times[95], times[99]
    );
    anyhow::ensure!(times[95] < 50.0, "render p95 over 50 ms");
    Ok(())
}
